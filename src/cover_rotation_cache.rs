//! Write-side invalidation for automatic book covers. All catalog/source I/O stays on
//! a finite worker; the session stamp belongs only to accepted UI material.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, mpsc};

use crate::catalog::{CacheEntry, CatalogWork, ThumbnailHeader};
use crate::cover_auto_material::AutoMaterial;
use crate::cover_identity::CoverIdentity;
use crate::grid_item::GridItem;

pub(crate) type CacheMap = Arc<RwLock<HashMap<String, CacheEntry>>>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum BookCoverId {
    Pdf(PathBuf),
    Zip {
        path: PathBuf,
        prefix: Option<String>,
    },
    Archive(PathBuf),
    Folder(PathBuf),
}

impl BookCoverId {
    fn normalized(self) -> Self {
        let normal = |path: PathBuf| PathBuf::from(crate::adjustment_db::normalize_path(&path));
        match self {
            Self::Pdf(path) => Self::Pdf(normal(path)),
            Self::Zip { path, prefix } => Self::Zip {
                path: normal(path),
                prefix: prefix.map(|prefix| prefix.to_lowercase()),
            },
            Self::Archive(path) => Self::Archive(normal(path)),
            Self::Folder(path) => Self::Folder(normal(path)),
        }
    }
    pub(crate) fn from_grid_item(item: &GridItem) -> Option<Self> {
        let path = |p: &Path| PathBuf::from(crate::adjustment_db::normalize_path(p));
        Some(match item {
            GridItem::PdfFile(p) | GridItem::PdfPage { pdf_path: p, .. } => Self::Pdf(path(p)),
            GridItem::ZipFile(p) | GridItem::ZipImage { zip_path: p, .. } => Self::Zip {
                path: path(p),
                prefix: None,
            },
            GridItem::ZipDir {
                zip_path,
                dir_prefix,
                ..
            } => Self::Zip {
                path: path(zip_path),
                prefix: Some(dir_prefix.to_lowercase()),
            },
            GridItem::ConvertibleArchive { path: p, .. } => Self::Archive(path(p)),
            GridItem::Folder(p) => Self::Folder(path(p)),
            GridItem::Image(p) => Self::Folder(path(p.parent()?)),
            GridItem::SearchContainer { path: p, kind, .. } => match kind {
                crate::grid_item::SearchContainerKind::Folder => Self::Folder(path(p)),
                crate::grid_item::SearchContainerKind::Zip => Self::Zip {
                    path: path(p),
                    prefix: None,
                },
            },
            _ => return None,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Pdf(p) | Self::Archive(p) | Self::Folder(p) | Self::Zip { path: p, .. } => p,
        }
    }

    /// Uses a known typed container; delimiters inside ZIP entries stay literal.
    pub(crate) fn contains_page_key(&self, key: &str) -> bool {
        match self {
            Self::Folder(p) => {
                let base = format!(
                    "{}/",
                    crate::adjustment_db::normalize_path(p).trim_end_matches('/')
                );
                // A direct Windows filename cannot contain the archive/page
                // separator. A K2 child book next to the folder's images is
                // still a different logical book, even without another '/'.
                key.strip_prefix(&base).is_some_and(|tail| {
                    !tail.is_empty() && !tail.contains('/') && !tail.contains("::")
                })
            }
            Self::Pdf(p) | Self::Archive(p) => {
                key.starts_with(&format!("{}::", crate::adjustment_db::normalize_path(p)))
            }
            Self::Zip { path, prefix } => key.starts_with(&crate::adjustment_db::zip_entry_key(
                path,
                prefix.as_deref().unwrap_or(""),
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AutoCoverSlot {
    pub(crate) logical_book: BookCoverId,
    pub(crate) representative_page_key: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CoverCacheStamp {
    #[default]
    Untracked,
    Tracked {
        slot: Arc<AutoCoverSlot>,
        serial: u64,
    },
}

impl CoverCacheStamp {
    pub(crate) fn slot(&self) -> Option<&AutoCoverSlot> {
        match self {
            Self::Tracked { slot, .. } => Some(slot),
            Self::Untracked => None,
        }
    }
}

/// Alias paths are physical converted ZIPs, never independent logical owners.
#[derive(Clone)]
pub(crate) struct SourceHint {
    pub(crate) book: BookCoverId,
    pub(crate) aliases: Vec<PathBuf>,
    pub(crate) representative_page_keys: Vec<String>,
    pub(crate) selection: Option<SelectionHint>,
}

#[derive(Clone)]
pub(crate) struct SelectionHint {
    pub(crate) request: crate::thumb_loader::LoadRequest,
    pub(crate) pin_db: Option<Arc<crate::folder_thumb_pins::FolderThumbPinDb>>,
}

impl SourceHint {
    pub(crate) fn new(book: BookCoverId) -> Self {
        Self {
            book: book.normalized(),
            aliases: Vec::new(),
            representative_page_keys: Vec::new(),
            selection: None,
        }
    }
    pub(crate) fn contains_page_key(&self, key: &str) -> bool {
        self.book.contains_page_key(key)
            || self.aliases.iter().any(|p| {
                let id = match &self.book {
                    BookCoverId::Zip { prefix, .. } => BookCoverId::Zip {
                        path: p.clone(),
                        prefix: prefix.clone(),
                    },
                    BookCoverId::Pdf(_) => BookCoverId::Pdf(p.clone()),
                    _ => BookCoverId::Archive(p.clone()),
                };
                id.contains_page_key(key)
            })
    }
}

#[derive(Clone)]
pub(crate) struct CatalogScope {
    pub(crate) folder: PathBuf,
    pub(crate) memory: Vec<CacheMap>,
}

pub(crate) struct InvalidationRequest {
    pub(crate) changed_keys: Vec<String>,
    pub(crate) source_hints: Vec<SourceHint>,
    pub(crate) scopes: Vec<CatalogScope>,
    pub(crate) work: CatalogWork,
    pub(crate) archive_cache: Option<Arc<crate::archive_cache::ArchiveCacheDb>>,
}

#[derive(Default)]
pub(crate) struct InvalidationCompletion {
    pub(crate) affected_slots: Vec<AutoCoverSlot>,
    /// Current request selection only; historical pinned/sorted rows must not
    /// cause a cover-only pending request to be canceled.
    pub(crate) pending_slots: Vec<AutoCoverSlot>,
    pub(crate) deleted_keys: Vec<(PathBuf, Vec<String>)>,
    pub(crate) errors: Vec<String>,
}

pub(crate) struct CoverRotationCache {
    serials: HashMap<AutoCoverSlot, u64>,
    sender: mpsc::Sender<InvalidationCompletion>,
    receiver: mpsc::Receiver<InvalidationCompletion>,
}

impl Default for CoverRotationCache {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            serials: HashMap::new(),
            sender,
            receiver,
        }
    }
}

impl CoverRotationCache {
    #[cfg(test)]
    pub(crate) fn complete_for_test(&self, completion: InvalidationCompletion) {
        self.sender.send(completion).unwrap();
    }
    pub(crate) fn stamp(&self, slot: AutoCoverSlot) -> CoverCacheStamp {
        let serial = self.serials.get(&slot).copied().unwrap_or(0);
        CoverCacheStamp::Tracked {
            slot: Arc::new(slot),
            serial,
        }
    }
    pub(crate) fn is_current(&self, stamp: &CoverCacheStamp) -> bool {
        match stamp {
            CoverCacheStamp::Untracked => true,
            CoverCacheStamp::Tracked { slot, serial } => {
                self.serials.get(slot.as_ref()).copied().unwrap_or(0) == *serial
            }
        }
    }
    pub(crate) fn submit(&self, request: InvalidationRequest, wake: Option<egui::Context>) {
        let sender = self.sender.clone();
        let fallback = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("cover-cache-delete".into())
            .spawn(move || {
                let _ = sender.send(run_invalidation(request));
                if let Some(ctx) = wake {
                    ctx.request_repaint();
                }
            })
        {
            let _ = fallback.send(InvalidationCompletion {
                errors: vec![error.to_string()],
                ..Default::default()
            });
        }
    }
    pub(crate) fn poll(&mut self) -> Vec<InvalidationCompletion> {
        let completions: Vec<_> = self.receiver.try_iter().collect();
        for completion in &completions {
            for slot in &completion.affected_slots {
                *self.serials.entry(slot.clone()).or_default() += 1;
            }
        }
        completions
    }
}

fn header_from_memory(filename: &str, row: &CacheEntry) -> ThumbnailHeader {
    ThumbnailHeader {
        filename: filename.to_owned(),
        cover: row.cover.clone(),
        auto_material: row.auto_material,
        folder_provenance: row.folder_provenance,
        selection_proof: row.selection_proof.clone(),
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    crate::path_key::eq_keep_drive(a, b)
}

fn source_paths(hint: &SourceHint) -> impl Iterator<Item = &Path> {
    std::iter::once(hint.book.path()).chain(hint.aliases.iter().map(PathBuf::as_path))
}

fn basename_scope(scope: &Path, path: &Path) -> bool {
    path.parent().is_some_and(|parent| same_path(scope, parent))
}

fn valid_generation(value: &str) -> bool {
    value
        .split_once(':')
        .is_some_and(|(mtime, size)| mtime.parse::<i64>().is_ok() && size.parse::<i64>().is_ok())
}

fn matches_base_or_generation(base: &str, expected: &str) -> bool {
    if base == expected {
        return true;
    }
    base.strip_prefix(expected)
        .and_then(|tail| tail.strip_prefix("|generation:"))
        .is_some_and(valid_generation)
}

fn matches_derived_source(base: &str, pin: &str, source: &str) -> bool {
    // A ZIP directory or explicit search page may contain a literal #pin:.
    // Reproduce the existing builder position using the known typed source,
    // rather than treating the first delimiter as the source boundary.
    let expected = crate::thumb_loader::representative_cache_key_for_policy_with_auto(
        source,
        &Default::default(),
        Some(crate::cover_auto_crop::AutoCropIdentity::new(
            crate::settings::ReadingDirection::Rtl,
        )),
    );
    let Some((expected_base, _, source_tail)) =
        crate::thumb_loader::representative_auto_crop_key_parts(&expected)
    else {
        return false;
    };
    if base != expected_base.to_lowercase().replace('\\', "/") {
        return false;
    }
    if source_tail.is_empty() {
        return true;
    }
    let pin = pin.to_lowercase();
    let source_tail = source_tail.to_lowercase();
    pin.strip_prefix(&source_tail).is_some_and(|tail| {
        tail.is_empty()
            || tail
                .strip_prefix(crate::thumb_loader::CACHE_KEY_PIN_SUFFIX)
                .is_some_and(|tail| !tail.is_empty())
            || tail
                .strip_prefix("|generation:")
                .is_some_and(valid_generation)
    })
}

/// A known book supplies the source boundary. Never split an entry/path on
/// arbitrary `::`, `#pin:` or `|generation:` and guess which part was a source.
fn row_belongs_to(hint: &SourceHint, scope: &Path, header: &ThumbnailHeader) -> bool {
    row_belongs_to_with_slot(hint, scope, header, false)
}

fn row_belongs_to_with_slot(
    hint: &SourceHint,
    scope: &Path,
    header: &ThumbnailHeader,
    known_slot: bool,
) -> bool {
    if !matches!(header.auto_material, AutoMaterial::Automatic(_)) {
        return false;
    }
    let CoverIdentity::Page(representative) = &header.cover else {
        return false;
    };
    if !known_slot && !hint.contains_page_key(representative) {
        return false;
    }
    let Some((base, token, pin)) =
        crate::thumb_loader::representative_auto_crop_key_parts(&header.filename)
    else {
        return false;
    };
    if !["|autocrop:v4:", "|autocrop:v4f:"]
        .iter()
        .any(|version| token.starts_with(version))
    {
        return false;
    }
    let base = base
        .rsplit_once("|types:")
        .filter(|(_, digest)| {
            digest.len() == 16 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .map_or(base, |(base, _)| base);
    let base = base.to_lowercase().replace('\\', "/");
    if let BookCoverId::Folder(folder) = &hint.book {
        // K2 parents can carry a child's Automatic. Only a direct image selected
        // by this exact directory is its own Automatic cover.
        let CoverIdentity::Page(cover) = &header.cover else {
            return false;
        };
        if !hint.book.contains_page_key(cover) {
            return false;
        }
        if header.folder_provenance == Some(crate::catalog::FolderThumbProvenance::Seeded) {
            return false;
        }
        if let Some(proof) = &header.selection_proof {
            if proof.winner.archive_row_key.is_some()
                || !proof
                    .winner
                    .path
                    .parent()
                    .is_some_and(|parent| same_path(parent, folder))
                || crate::adjustment_db::normalize_path(&proof.winner.path) != *cover
            {
                return false;
            }
        }
        let Some(body) = base.strip_prefix("folderthumb:auto-v") else {
            return false;
        };
        let Some((version, body)) = body.split_once(':') else {
            return false;
        };
        if version != "2" && version != "3" {
            return false;
        }
        let Some((sort, body)) = body.split_once(':') else {
            return false;
        };
        let sort = match sort {
            "name" => crate::settings::SortOrder::FileName,
            "numeric" => crate::settings::SortOrder::Numeric,
            "date-asc" => crate::settings::SortOrder::DateAsc,
            "date-desc" => crate::settings::SortOrder::DateDesc,
            _ => return false,
        };
        let Some((depth, _)) = body.split_once(':') else {
            return false;
        };
        let Some(depth) = depth
            .strip_prefix('d')
            .and_then(|depth| depth.parse::<u32>().ok())
        else {
            return false;
        };
        let provenance = if version == "2" {
            crate::catalog::FolderThumbProvenance::Seeded
        } else {
            crate::catalog::FolderThumbProvenance::AutoSelected
        };
        return [true, false].into_iter().any(|full| {
            (full || basename_scope(scope, folder))
                && crate::thumb_loader::folder_thumb_cache_key_for_path(
                    folder, full, sort, depth, provenance,
                )
                .is_some_and(|expected| base == expected.to_lowercase().replace('\\', "/"))
        });
    }
    if let BookCoverId::Zip {
        prefix: Some(prefix),
        ..
    } = &hint.book
    {
        return (source_paths(hint).any(|path| same_path(path, scope))
            || (known_slot
                && representative
                    .starts_with(&crate::adjustment_db::zip_entry_key(scope, prefix))))
            && matches_derived_source(&base, pin, &crate::grid_item::zipdir_cache_key(prefix));
    }
    let prefix = match &hint.book {
        BookCoverId::Pdf(_) => crate::thumb_loader::CACHE_KEY_PDF,
        BookCoverId::Zip { .. } => crate::thumb_loader::CACHE_KEY_ZIP,
        BookCoverId::Archive(_) => crate::thumb_loader::CACHE_KEY_ARCHIVE,
        BookCoverId::Folder(_) => unreachable!(),
    };
    source_paths(hint).any(|path| {
        let full = crate::adjustment_db::normalize_path(path);
        if matches_base_or_generation(&base, &format!("{prefix}{full}")) {
            return true;
        }
        if basename_scope(scope, path)
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    matches_base_or_generation(&base, &format!("{prefix}{}", name.to_lowercase()))
                })
        {
            return true;
        }
        // Global search representatives contain an explicitly selected page.
        // Validate the whole known source/page key rather than parsing its text.
        let CoverIdentity::Page(cover) = &header.cover else {
            return false;
        };
        if !hint.contains_page_key(cover) {
            return false;
        }
        if let Some(entry) = cover.strip_prefix(&format!("{full}::")) {
            if matches!(&hint.book, BookCoverId::Pdf(_)) {
                if let Some(page) = entry
                    .strip_prefix("page_")
                    .and_then(|page| page.parse::<u32>().ok())
                {
                    return matches_derived_source(
                        &base,
                        pin,
                        &format!(
                            "{}{full}#p{page}",
                            crate::thumb_loader::CACHE_KEY_SEARCH_REP
                        ),
                    );
                }
            } else {
                return matches_derived_source(
                    &base,
                    pin,
                    &format!(
                        "{}{full}::{entry}",
                        crate::thumb_loader::CACHE_KEY_SEARCH_REP
                    ),
                );
            }
        }
        false
    })
}

fn affected_slot(
    hint: &SourceHint,
    changed: &[String],
    representative: &str,
) -> Option<AutoCoverSlot> {
    changed
        .iter()
        .any(|key| key != representative && hint.contains_page_key(key))
        .then(|| AutoCoverSlot {
            logical_book: hint.book.clone(),
            representative_page_key: representative.to_owned(),
        })
}

fn selected_representative(hint: &SourceHint) -> Option<String> {
    let selection = hint.selection.as_ref()?;
    let mut resolved_request = selection.request.clone();
    if resolved_request.zip_entry.is_none() && resolved_request.pdf_page.is_none() {
        let container = match &hint.book {
            BookCoverId::Zip {
                path,
                prefix: Some(prefix),
            } => crate::app::book_container_key(path, prefix),
            _ => hint.book.path().to_owned(),
        };
        if let Some(pin_db) = &selection.pin_db {
            if let Some(pin) = pin_db.lookup(&container) {
                if let Some(target) = crate::folder_thumb_pins::resolve_pin_target_cascaded_via(
                    &container,
                    &pin,
                    |path| pin_db.lookup(path),
                    resolved_request.folder_thumb_depth as usize,
                )
                .filter(|target| {
                    crate::thumb_loader::resolved_pin_allowed(
                        target,
                        &resolved_request.file_type_visibility,
                    )
                }) {
                    if matches!(&hint.book, BookCoverId::Folder(_)) {
                        return None;
                    }
                    use crate::folder_thumb_pins::ResolvedKind;
                    target.assign_to_load_request(
                        &mut resolved_request,
                        "cover-rotation-selection",
                        crate::folder_thumb_pins::PinKeyMode::Exact,
                        None,
                    );
                    if resolved_request.cover_crop.is_some() {
                        return None;
                    }
                    match target.kind {
                        ResolvedKind::PdfFirstPage | ResolvedKind::PdfPage => {
                            return Some(crate::edit_source::page_key_for_pdf(
                                &target.abs_path,
                                target.pdf_page.unwrap_or(0),
                            ));
                        }
                        ResolvedKind::ZipEntry => {
                            let entry = target.zip_entry.as_deref()?;
                            crate::thumb_loader::automatic_container_source(
                                &target.abs_path,
                                Some(entry),
                                None,
                            )
                            .ok()?;
                            return Some(crate::adjustment_db::zip_entry_key(
                                &target.abs_path,
                                entry,
                            ));
                        }
                        ResolvedKind::ZipFirstImage
                        | ResolvedKind::ArchiveFirstImage
                        | ResolvedKind::ZipDirRepresentative => {
                            resolved_request.path = target.abs_path;
                            resolved_request.zip_dir_prefix = target.zip_dir_prefix;
                        }
                        _ => return None,
                    }
                }
            }
        }
        if (matches!(&hint.book, BookCoverId::Archive(_))
            || matches!(
                &hint.book,
                BookCoverId::Zip {
                    prefix: Some(_),
                    ..
                }
            ) && !hint.aliases.is_empty())
            && same_path(&resolved_request.path, hint.book.path())
        {
            resolved_request.path = hint.aliases.first()?.clone();
        }
    }
    let req = &resolved_request;
    // These are admitted request facts, including the effective pin, not a new
    // parse of the derived cache key. Manual/pinned-folder material is excluded.
    if req.auto_cover_crop.is_none() || req.cover_crop.is_some() {
        return None;
    }
    if let Some(page) = req.pdf_page {
        return Some(crate::edit_source::page_key_for_pdf(&req.path, page));
    }
    if let Some(entry) = &req.zip_entry {
        crate::thumb_loader::automatic_container_source(&req.path, Some(entry), None).ok()?;
        return Some(crate::adjustment_db::zip_entry_key(&req.path, entry));
    }
    if let Some(prefix) = &req.zip_dir_prefix {
        let entries = crate::zip_loader::enumerate_image_entries_detailed(&req.path)
            .ok()?
            .entries;
        let tree = crate::zip_tree::ZipTree::build(req.path.clone(), entries)
            .project(&req.file_type_visibility);
        let entry = tree.representative_for_prefix_str(
            prefix,
            req.folder_thumb_sort
                .unwrap_or(crate::settings::SortOrder::Numeric),
        )?;
        crate::thumb_loader::automatic_container_source(&req.path, Some(&entry.entry_name), None)
            .ok()?;
        return Some(crate::adjustment_db::zip_entry_key(
            &req.path,
            &entry.entry_name,
        ));
    }
    match &hint.book {
        BookCoverId::Pdf(path) => Some(crate::edit_source::page_key_for_pdf(path, 0)),
        BookCoverId::Zip {
            prefix: Some(prefix),
            ..
        } => {
            let prefix = req.zip_dir_prefix.as_deref().unwrap_or(prefix);
            let entries = crate::zip_loader::enumerate_image_entries_detailed(&req.path)
                .ok()?
                .entries;
            let tree = crate::zip_tree::ZipTree::build(req.path.clone(), entries)
                .project(&req.file_type_visibility);
            let entry = tree.representative_for_prefix_str(
                prefix,
                req.folder_thumb_sort
                    .unwrap_or(crate::settings::SortOrder::Numeric),
            )?;
            crate::thumb_loader::automatic_container_source(
                &req.path,
                Some(&entry.entry_name),
                None,
            )
            .ok()?;
            Some(crate::adjustment_db::zip_entry_key(
                &req.path,
                &entry.entry_name,
            ))
        }
        BookCoverId::Zip { .. } | BookCoverId::Archive(_) => {
            let entry = crate::zip_loader::first_visible_image_entry(
                &req.path,
                None,
                &req.file_type_visibility,
            )?;
            crate::thumb_loader::automatic_container_source(&req.path, Some(&entry), None).ok()?;
            Some(crate::adjustment_db::zip_entry_key(&req.path, &entry))
        }
        BookCoverId::Folder(path) => {
            if !req.auto_cover_crop.is_some_and(|auto| auto.folders)
                || req.folder_thumb_provenance
                    == Some(crate::catalog::FolderThumbProvenance::Seeded)
                || !same_path(&req.path, path)
            {
                return None;
            }
            crate::thumb_loader::direct_folder_representative_key(
                path,
                req.folder_thumb_sort
                    .unwrap_or(crate::settings::SortOrder::Numeric),
                &req.file_type_visibility,
            )
        }
    }
}

fn report_catalog_error(
    work: &CatalogWork,
    completion: &mut InvalidationCompletion,
    scope: &Path,
    error: rusqlite::Error,
) {
    if work.access().is_admitted(work.identity().1) {
        completion
            .errors
            .push(format!("{}: {error}", scope.display()));
    } else {
        crate::logger::log(format!(
            "cover rotation cache: retired catalog skipped {}: {error}",
            scope.display()
        ));
    }
}

/// Pure, scope-qualified cleanup for memory created after the worker captured
/// its maps, including a retained grid first restored after completion.
pub(crate) fn remove_invalidated_memory_rows(
    scope: &Path,
    memory: &CacheMap,
    slots: &[AutoCoverSlot],
    hints: &[SourceHint],
) -> usize {
    let mut map = memory.write().unwrap();
    let before = map.len();
    map.retain(|filename, row| {
        let CoverIdentity::Page(representative) = &row.cover else {
            return true;
        };
        if !slots
            .iter()
            .any(|slot| representative == &slot.representative_page_key)
        {
            return true;
        }
        let header = header_from_memory(filename, row);
        !slots.iter().any(|slot| {
            if representative != &slot.representative_page_key {
                return false;
            }
            let fallback = SourceHint::new(slot.logical_book.clone());
            let matching: Vec<_> = hints
                .iter()
                .filter(|hint| hint.book == slot.logical_book)
                .collect();
            if matching.is_empty() {
                row_belongs_to_with_slot(&fallback, scope, &header, true)
            } else {
                matching
                    .iter()
                    .any(|hint| row_belongs_to_with_slot(hint, scope, &header, true))
            }
        })
    });
    before - map.len()
}

fn source_hint_for_existing_page_key(key: &str) -> Option<SourceHint> {
    // Reuse the existing worker restoration route, which establishes physical
    // existence, recognized source kind and ZIP-entry identity. A delimiter or
    // an extension by itself never establishes an additional logical owner.
    let restored = crate::rating_view::rating_row_to_view_row(&crate::rating_db::RatingRow {
        key: key.to_owned(),
        stars: 0,
        rated_at_ms: None,
        source_path: None,
        kind: None,
        entry_name: None,
        page_num: None,
        dir_prefix: None,
        archive_format: None,
        zipdir_is_archive: None,
        zipdir_representative: None,
    })?;
    match &restored.item {
        GridItem::PdfPage { .. } | GridItem::ZipImage { .. } | GridItem::Image(_) => {
            BookCoverId::from_grid_item(&restored.item).map(SourceHint::new)
        }
        _ => None,
    }
}

/// Synchronous worker body is exposed internally for isolated synthetic tests.
pub(crate) fn run_invalidation(mut request: InvalidationRequest) -> InvalidationCompletion {
    let started = std::time::Instant::now();
    let mut completion = InvalidationCompletion::default();
    crate::logger::log(format!(
        "cover rotation cache: begin keys={} sources={} scopes={}",
        request.changed_keys.len(),
        request.source_hints.len(),
        request.scopes.len()
    ));
    for key in &request.changed_keys {
        // A mounted ZipDir is not the whole book. Existing typed alias facts can
        // establish its outer owner without reinterpreting any entry delimiter.
        let roots: Vec<_> = request
            .source_hints
            .iter()
            .filter(|hint| hint.contains_page_key(key))
            .filter_map(|hint| {
                let BookCoverId::Zip {
                    path,
                    prefix: Some(_),
                } = &hint.book
                else {
                    return None;
                };
                let mut root = hint.clone();
                root.book = if hint.aliases.is_empty() {
                    BookCoverId::Zip {
                        path: path.clone(),
                        prefix: None,
                    }
                } else {
                    BookCoverId::Archive(path.clone())
                };
                root.representative_page_keys.clear();
                root.selection = None;
                Some(root)
            })
            .collect();
        for root in roots {
            if !request
                .source_hints
                .iter()
                .any(|hint| hint.book == root.book)
            {
                request.source_hints.push(root);
            }
        }
        if let Some(mut hint) = source_hint_for_existing_page_key(key) {
            if let Some(owner) = request.source_hints.iter().find(|owner| {
                owner
                    .aliases
                    .iter()
                    .any(|alias| same_path(alias, hint.book.path()))
            }) {
                hint.book = match &owner.book {
                    BookCoverId::Zip {
                        path,
                        prefix: Some(_),
                        ..
                    } => BookCoverId::Archive(path.clone()),
                    _ => owner.book.clone(),
                };
                hint.aliases = owner.aliases.clone();
            }
            if !request
                .source_hints
                .iter()
                .any(|existing| existing.book == hint.book)
            {
                request.source_hints.push(hint);
            }
        } else if !request
            .source_hints
            .iter()
            .any(|hint| hint.contains_page_key(key))
        {
            crate::logger::log(format!(
                "cover rotation cache: could not resolve changed page {key}"
            ));
        }
    }
    let mut merged: Vec<SourceHint> = Vec::new();
    let mut first_by_book = HashMap::new();
    for hint in request.source_hints.drain(..) {
        if hint.selection.is_none() {
            if let Some(&index) = first_by_book.get(&hint.book) {
                let existing: &mut SourceHint = &mut merged[index];
                for alias in hint.aliases {
                    if !existing.aliases.iter().any(|old| same_path(old, &alias)) {
                        existing.aliases.push(alias);
                    }
                }
                for representative in hint.representative_page_keys {
                    if !existing.representative_page_keys.contains(&representative) {
                        existing.representative_page_keys.push(representative);
                    }
                }
                continue;
            }
        }
        first_by_book
            .entry(hint.book.clone())
            .or_insert(merged.len());
        merged.push(hint);
    }
    request.source_hints = merged;
    if let Some(cache) = &request.archive_cache {
        for hint in &mut request.source_hints {
            if !matches!(
                &hint.book,
                BookCoverId::Archive(_) | BookCoverId::Zip { .. }
            ) {
                continue;
            }
            let snapshot = cache
                .snapshot_for_source(hint.book.path())
                .or_else(|| cache.snapshot_for_cached_zip(hint.book.path()));
            if let Some(snapshot) = snapshot.filter(|snapshot| snapshot.is_current()) {
                if !hint
                    .aliases
                    .iter()
                    .any(|alias| same_path(alias, &snapshot.cached_zip_path))
                {
                    hint.aliases.push(snapshot.cached_zip_path);
                }
                hint.book = match &hint.book {
                    BookCoverId::Zip {
                        prefix: Some(prefix),
                        ..
                    } => BookCoverId::Zip {
                        path: PathBuf::from(crate::adjustment_db::normalize_path(
                            &snapshot.source_path,
                        )),
                        prefix: Some(prefix.clone()),
                    },
                    _ => BookCoverId::Archive(PathBuf::from(crate::adjustment_db::normalize_path(
                        &snapshot.source_path,
                    ))),
                };
            }
        }
    }
    // Different contexts may retain different pin/request selections for the
    // same book. Their physical aliases are source facts shared by all those
    // selections, including a Loaded material which was never saved to disk.
    let alias_owner = |book: &BookCoverId| match book {
        BookCoverId::Zip { path, .. } | BookCoverId::Archive(path) => {
            BookCoverId::Archive(path.clone())
        }
        other => other.clone(),
    };
    let mut aliases: HashMap<BookCoverId, Vec<PathBuf>> = HashMap::new();
    for hint in &request.source_hints {
        let known = aliases.entry(alias_owner(&hint.book)).or_default();
        for alias in &hint.aliases {
            if !known.iter().any(|path| same_path(path, alias)) {
                known.push(alias.clone());
            }
        }
    }
    for hint in &mut request.source_hints {
        hint.aliases = aliases
            .get(&alias_owner(&hint.book))
            .cloned()
            .unwrap_or_default();
    }
    request.source_hints.retain(|hint| {
        request
            .changed_keys
            .iter()
            .any(|key| hint.contains_page_key(key))
    });
    // A body page belongs to the whole ZIP and every containing ZipDir. These
    // are logical books, not ancestor physical folders. Derive only prefixes of
    // entries inside a known typed outer container, never by splitting a raw
    // page key at its first delimiter.
    let mut dirs = Vec::new();
    for hint in &request.source_hints {
        if !matches!(
            &hint.book,
            BookCoverId::Zip { prefix: None, .. } | BookCoverId::Archive(_)
        ) {
            continue;
        }
        for path in source_paths(hint) {
            let base = crate::adjustment_db::zip_entry_key(path, "");
            for changed in &request.changed_keys {
                if let Some(entry) = changed.strip_prefix(&base) {
                    for (offset, _) in entry.match_indices('/') {
                        let prefix = entry[..=offset].to_owned();
                        let mut derived = hint.clone();
                        derived.book = BookCoverId::Zip {
                            path: hint.book.path().to_owned(),
                            prefix: Some(prefix.clone()),
                        };
                        derived.representative_page_keys.clear();
                        if let Some(selection) = &mut derived.selection {
                            selection.request.zip_entry = None;
                            selection.request.pdf_page = None;
                            selection.request.zip_dir_prefix = Some(prefix);
                            selection.request.path = path.to_owned();
                        }
                        if !request
                            .source_hints
                            .iter()
                            .any(|existing| existing.book == derived.book)
                            && !dirs
                                .iter()
                                .any(|existing: &SourceHint| existing.book == derived.book)
                        {
                            dirs.push(derived);
                        }
                    }
                }
            }
        }
    }
    request.source_hints.extend(dirs);
    let mut inferred_scopes = Vec::new();
    for hint in &request.source_hints {
        for path in source_paths(hint) {
            if let Some(parent) = path.parent() {
                inferred_scopes.push(parent.to_owned());
                if let Some(collection) = crate::catalog::collection_source_parent_identity(parent)
                {
                    inferred_scopes.push(collection);
                }
            }
            if matches!(
                &hint.book,
                BookCoverId::Zip { .. } | BookCoverId::Archive(_)
            ) {
                inferred_scopes.push(path.to_owned());
            }
        }
    }
    for folder in inferred_scopes {
        if !request
            .scopes
            .iter()
            .any(|scope| same_path(&scope.folder, &folder))
        {
            request.scopes.push(CatalogScope {
                folder,
                memory: Vec::new(),
            });
        }
    }
    let hints: Vec<_> = request
        .source_hints
        .iter()
        .filter(|hint| {
            request
                .changed_keys
                .iter()
                .any(|key| hint.contains_page_key(key))
        })
        .collect();
    let mut affected = HashSet::new();
    let mut pending = HashSet::new();
    for hint in &hints {
        for representative in &hint.representative_page_keys {
            if !hint.contains_page_key(representative) {
                continue;
            }
            if let Some(slot) = affected_slot(hint, &request.changed_keys, representative) {
                affected.insert(slot);
            }
        }
        if let Some(representative) =
            selected_representative(hint).filter(|page| hint.contains_page_key(page))
        {
            if let Some(slot) = affected_slot(hint, &request.changed_keys, &representative) {
                affected.insert(slot.clone());
                pending.insert(slot);
            }
        } else if hint.selection.is_some() {
            crate::logger::log(format!(
                "cover rotation cache: no automatic current representative for {} (unreadable or ineligible)",
                hint.book.path().display()
            ));
        }
    }
    for scope in &request.scopes {
        // Snapshot memory headers, then release all map locks before SQL.
        let memory_headers: Vec<_> = scope
            .memory
            .iter()
            .flat_map(|map| {
                map.read()
                    .unwrap()
                    .iter()
                    .map(|(key, row)| header_from_memory(key, row))
                    .collect::<Vec<_>>()
            })
            .collect();
        let db = match request.work.open_existing_for_update(&scope.folder) {
            Ok(db) => {
                // Collection's storage identity is synthetic, but its q: cover
                // identities still belong to the physical source drive.
                let source_parent = hints
                    .iter()
                    .flat_map(|hint| source_paths(hint))
                    .filter_map(Path::parent)
                    .find(|parent| {
                        crate::catalog::collection_source_parent_identity(parent)
                            .is_some_and(|identity| same_path(&identity, &scope.folder))
                    });
                db.map(|db| match source_parent {
                    Some(parent) => db.with_cover_source_folder(parent),
                    None => db,
                })
            }
            Err(error) => {
                report_catalog_error(&request.work, &mut completion, &scope.folder, error);
                None
            }
        };
        let mut headers = memory_headers;
        if let Some(db) = &db {
            match db.thumbnail_headers() {
                Ok(rows) => headers.extend(rows),
                Err(error) => {
                    report_catalog_error(&request.work, &mut completion, &scope.folder, error)
                }
            }
        }
        let mut deletes = HashSet::new();
        for header in &headers {
            let CoverIdentity::Page(representative) = &header.cover else {
                continue;
            };
            for hint in &hints {
                if row_belongs_to(hint, &scope.folder, header) {
                    if let Some(slot) = affected_slot(hint, &request.changed_keys, representative) {
                        affected.insert(slot);
                        deletes.insert(header.filename.clone());
                    }
                }
            }
        }
        let mut deletes: Vec<_> = deletes.into_iter().collect();
        deletes.sort();
        if !deletes.is_empty() {
            if let Some(db) = &db {
                if let Err(error) = db.delete_exact_keys(&deletes) {
                    report_catalog_error(&request.work, &mut completion, &scope.folder, error);
                }
            }
            for map in &scope.memory {
                let mut map = map.write().unwrap();
                for key in &deletes {
                    map.remove(key);
                }
            }
            completion
                .deleted_keys
                .push((scope.folder.clone(), deletes));
        }
    }
    completion.affected_slots = affected.into_iter().collect();
    completion.pending_slots = pending.into_iter().collect();
    for error in &completion.errors {
        request
            .work
            .access()
            .record_cache_error(format!("cover rotation cache: {error}"));
    }
    crate::logger::log(format!(
        "cover rotation cache: complete slots={} catalogs={} errors={}; last writer races require manual cache rebuild",
        completion.affected_slots.len(),
        completion.deleted_keys.len(),
        completion.errors.len()
    ));
    crate::perf::event(
        "thumb",
        "cover_rotation_invalidation",
        None,
        0,
        &[
            ("ms", (started.elapsed().as_secs_f64() * 1000.0).into()),
            ("changed_keys", request.changed_keys.len().into()),
            ("targeted_slots", completion.affected_slots.len().into()),
            (
                "deleted_keys",
                completion
                    .deleted_keys
                    .iter()
                    .map(|(_, keys)| keys.len())
                    .sum::<usize>()
                    .into(),
            ),
            ("catalogs", completion.deleted_keys.len().into()),
            ("errors", completion.errors.len().into()),
        ],
    );
    completion
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cover_auto_material::AutoCropResults;

    fn automatic() -> AutoMaterial {
        AutoMaterial::Automatic(AutoCropResults::new([None; 4]))
    }
    fn header(key: &str, cover: &str) -> ThumbnailHeader {
        ThumbnailHeader {
            filename: key.into(),
            cover: CoverIdentity::Page(cover.into()),
            auto_material: automatic(),
            folder_provenance: None,
            selection_proof: None,
        }
    }
    fn zip_hint(path: &Path) -> SourceHint {
        SourceHint::new(BookCoverId::Zip {
            path: path.to_owned(),
            prefix: None,
        })
    }
    fn request(
        cache: &Path,
        changed: &str,
        hints: Vec<SourceHint>,
        scopes: Vec<CatalogScope>,
    ) -> InvalidationRequest {
        InvalidationRequest {
            work: CatalogWork::capture(cache),
            changed_keys: vec![changed.into()],
            source_hints: hints,
            scopes,
            archive_cache: None,
        }
    }
    fn entry(cover: &str) -> CacheEntry {
        CacheEntry {
            mtime: 1,
            file_size: 2,
            jpeg_data: vec![1],
            cover: CoverIdentity::Page(cover.into()),
            auto_material: automatic(),
            source_dims: None,
            layout_dims: None,
            folder_provenance: None,
            selection_proof: None,
        }
    }

    #[test]
    fn cover_rotation_strict_existing_keys_preserve_types_pin_and_generation() {
        let path = Path::new("c:/books/book.zip");
        let scope = Path::new("c:/books");
        let hint = zip_hint(path);
        let cover = "c:/books/book.zip::cover.jpg";
        for key in [
            "zipthumb:book.zip|autocrop:v4:rtl",
            "zipthumb:book.zip|types:0123456789abcdef|autocrop:v4:ltr#pin:entry::|generation:7:9",
            "zipthumb:c:/books/book.zip|autocrop:v4:ltr#pin:old-page",
            "zipthumb:book.zip|generation:7:9|types:0123456789abcdef|autocrop:v4:rtl",
        ] {
            assert!(row_belongs_to(&hint, scope, &header(key, cover)), "{key}");
        }
        for key in [
            "book.zip|autocrop:v4:rtl",
            "page.jpg|autocrop:v4:rtl",
            "zipthumb:book.zip-more|autocrop:v4:rtl",
            "zipthumb:book.zip|types:not-a-digest|autocrop:v4:rtl",
            "zipthumb:book.zip|autocrop:v4:rtl/",
            "zipthumb:book.zip|autocrop:v4f:rtl",
            "zipthumb:book.zip|generation:entry:literal|autocrop:v4:rtl",
            "zipthumb:book.zip|autocrop:v1:rtl",
            "zipthumb:book.zip|autocrop:v4:rtl|generation:7:9",
            "zipthumb:book.zip|autocrop:v3:rtl",
            "folderthumb:book|autocrop:v3f:ltr",
        ] {
            assert!(!row_belongs_to(&hint, scope, &header(key, cover)), "{key}");
        }
        assert!(!row_belongs_to(
            &hint,
            Path::new("c:/another"),
            &header("zipthumb:book.zip|autocrop:v4:rtl", cover)
        ));
        let mut manual = header("zipthumb:book.zip|autocrop:v4:rtl", cover);
        manual.auto_material = AutoMaterial::NotAutomatic;
        assert!(!row_belongs_to(&hint, scope, &manual));
    }

    #[test]
    fn cover_rotation_zip_entry_delimiters_never_determine_book_identity() {
        let hint = zip_hint(Path::new("c:/books/weird::name.zip"));
        let cover = "c:/books/weird::name.zip::zipthumb:literal|autocrop:v4:rtl#pin:page|generation:1:2.jpg";
        let changed = "c:/books/weird::name.zip::chapter/body.jpg";
        assert!(hint.contains_page_key(changed));
        assert!(affected_slot(&hint, &[changed.into()], cover).is_some());
        assert!(affected_slot(&hint, &[cover.into()], cover).is_none());
        assert!(!hint.contains_page_key("c:/books/weird::name.zip-extra::body.jpg"));
        assert!(!row_belongs_to(
            &hint,
            Path::new("c:/books/weird::name.zip"),
            &header(
                "zipthumb:literal|autocrop:v4:rtl#pin:page|generation:1:2.jpg",
                cover
            )
        ));
        let dir = SourceHint::new(BookCoverId::Zip {
            path: PathBuf::from("c:/books/weird::name.zip"),
            prefix: Some("chapter/".into()),
        });
        assert!(dir.contains_page_key(changed));
        assert!(!dir.contains_page_key("c:/books/weird::name.zip::chapters/body.jpg"));
        let dir_cover = "c:/books/weird::name.zip::chapter/cover.jpg";
        assert!(row_belongs_to(
            &dir,
            Path::new("c:/books/weird::name.zip"),
            &header("zipdir:chapter/|autocrop:v4:ltr", dir_cover)
        ));
        assert!(!row_belongs_to(
            &dir,
            Path::new("c:/books/another.zip"),
            &header("zipdir:chapter/|autocrop:v4:ltr", dir_cover)
        ));
        let literal = "chapter/#pin:literal::|generation:1:2/";
        let unusual = SourceHint::new(BookCoverId::Zip {
            path: PathBuf::from("c:/books/weird::name.zip"),
            prefix: Some(literal.into()),
        });
        let key = crate::thumb_loader::representative_cache_key_for_policy_with_auto(
            &crate::grid_item::zipdir_cache_key(literal),
            &Default::default(),
            Some(crate::cover_auto_crop::AutoCropIdentity::new(
                crate::settings::ReadingDirection::Rtl,
            )),
        );
        let cover = format!("c:/books/weird::name.zip::{literal}cover.jpg");
        assert!(row_belongs_to(
            &unusual,
            Path::new("c:/books/weird::name.zip"),
            &header(&key, &cover)
        ));
        let other = key.replace("#pin:literal", "#pin:other");
        assert!(!row_belongs_to(
            &unusual,
            Path::new("c:/books/weird::name.zip"),
            &header(&other, &cover)
        ));
    }

    #[test]
    fn cover_rotation_direct_folder_excludes_k2_parent_and_other_folders() {
        let folder = PathBuf::from("c:/books/direct");
        let hint = SourceHint::new(BookCoverId::Folder(folder.clone()));
        let key = crate::thumb_loader::folder_thumb_auto_cache_key_for_path(
            &folder,
            false,
            crate::settings::SortOrder::Numeric,
            19,
        )
        .unwrap();
        let row = header(
            &format!("{key}|types:0123456789abcdef|autocrop:v4f:ltr"),
            "c:/books/direct/cover.jpg",
        );
        assert!(row_belongs_to(&hint, Path::new("c:/books"), &row));
        let mut child = row.clone();
        child.cover = CoverIdentity::Page("c:/books/direct/child/book.zip::cover.jpg".into());
        assert!(!row_belongs_to(&hint, Path::new("c:/books"), &child));
        let parent = SourceHint::new(BookCoverId::Folder(PathBuf::from("c:/books")));
        assert!(!row_belongs_to(&parent, Path::new("c:/"), &row));
        assert!(hint.contains_page_key("c:/books/direct/body.jpg"));
        assert!(!hint.contains_page_key("c:/books/direct/child/body.jpg"));
        for page in [
            "c:/books/direct/book.zip::body.jpg",
            "c:/books/direct/book.pdf::page_1",
        ] {
            assert!(!hint.contains_page_key(page));
            let inherited_cover = "c:/books/direct/book.zip::cover.jpg";
            assert!(affected_slot(&hint, &[page.into()], inherited_cover).is_none());
        }
    }

    #[test]
    fn cover_rotation_pdf_epub_and_converted_archive_aliases_have_exact_owners() {
        for name in ["book.pdf", "book.epub"] {
            let path = PathBuf::from(format!("c:/books/{name}"));
            let hint = SourceHint::new(BookCoverId::Pdf(path.clone()));
            let row = header(
                &format!("pdfthumb:{name}|autocrop:v4:rtl#pin:pdf||page|3|7|9|generation:7:9"),
                &crate::edit_source::page_key_for_pdf(&path, 3),
            );
            assert!(row_belongs_to(&hint, Path::new("c:/books"), &row));
            assert!(hint.contains_page_key(&crate::edit_source::page_key_for_pdf(&path, 10)));
        }
        let mut archive = SourceHint::new(BookCoverId::Archive(PathBuf::from("c:/books/book.7z")));
        archive
            .aliases
            .push(PathBuf::from("c:/cache/generated.zip"));
        let row = header(
            "archivethumb:book.7z|autocrop:v4:ltr",
            "c:/cache/generated.zip::cover.jpg",
        );
        assert!(row_belongs_to(&archive, Path::new("c:/books"), &row));
        assert!(
            affected_slot(
                &archive,
                &["c:/cache/generated.zip::body.jpg".into()],
                "c:/cache/generated.zip::cover.jpg"
            )
            .is_some()
        );
        assert!(!archive.contains_page_key("c:/cache/another.zip::body.jpg"));
    }

    #[test]
    fn cover_rotation_worker_deletes_exact_rows_and_memory_and_preserves_representative() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let parent = temp.path().join("books");
        let path = parent.join("book.zip");
        let cover = crate::adjustment_db::zip_entry_key(&path, "cover.jpg");
        let changed = crate::adjustment_db::zip_entry_key(&path, "body.jpg");
        let db = crate::catalog::CatalogDb::open(&cache, &parent).unwrap();
        let deleted = "zipthumb:book.zip|autocrop:v4:rtl";
        let preserved = "zipthumb:book.zip|autocrop:v4:ltr#pin:body.jpg";
        let ordinary = "cover.jpg|autocrop:v4:rtl";
        for (key, represented) in [(deleted, &cover), (preserved, &changed), (ordinary, &cover)] {
            db.save(
                key,
                1,
                2,
                2,
                3,
                None,
                b"never decode",
                &CoverIdentity::Page(represented.clone()),
                automatic(),
            )
            .unwrap();
        }
        let memory = Arc::new(RwLock::new(HashMap::from([
            (deleted.into(), entry(&cover)),
            (preserved.into(), entry(&changed)),
            (ordinary.into(), entry(&cover)),
        ])));
        let completion = run_invalidation(request(
            &cache,
            &changed,
            vec![zip_hint(&path)],
            vec![CatalogScope {
                folder: parent.clone(),
                memory: vec![memory.clone()],
            }],
        ));
        assert!(completion.errors.is_empty(), "{:?}", completion.errors);
        assert_eq!(
            completion.deleted_keys,
            vec![(parent, vec![deleted.to_owned()])]
        );
        assert_eq!(completion.affected_slots.len(), 1);
        assert_eq!(completion.affected_slots[0].representative_page_key, cover);
        assert!(db.load_one(deleted).unwrap().is_none());
        for key in [preserved, ordinary] {
            assert!(db.load_one(key).unwrap().is_some());
            assert!(memory.read().unwrap().contains_key(key));
        }
        assert!(!memory.read().unwrap().contains_key(deleted));
    }

    #[test]
    fn cover_rotation_missing_catalog_still_invalidates_pending_slot_without_creating_data() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("absent");
        let path = temp.path().join("book.epub");
        let mut hint = SourceHint::new(BookCoverId::Pdf(path.clone()));
        hint.selection = Some(SelectionHint {
            request: crate::thumb_loader::LoadRequest {
                path: path.clone(),
                auto_cover_crop: Some(crate::cover_auto_crop::AutoCropIdentity::new(
                    crate::settings::ReadingDirection::Rtl,
                )),
                ..Default::default()
            },
            pin_db: None,
        });
        let cover = crate::edit_source::page_key_for_pdf(&path, 0);
        let completion = run_invalidation(request(
            &cache,
            &crate::edit_source::page_key_for_pdf(&path, 3),
            vec![hint.clone()],
            vec![CatalogScope {
                folder: temp.path().to_owned(),
                memory: vec![],
            }],
        ));
        assert!(completion.errors.is_empty());
        assert_eq!(completion.affected_slots.len(), 1);
        assert_eq!(completion.affected_slots[0].representative_page_key, cover);
        assert!(!cache.exists());
        let cover_only = run_invalidation(request(&cache, &cover, vec![hint], vec![]));
        assert!(cover_only.affected_slots.is_empty());
    }

    #[test]
    fn cover_rotation_unmounted_zipdir_rows_are_found_from_typed_whole_zip() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let path = temp.path().join("book.zip");
        let db = crate::catalog::CatalogDb::open(&cache, &path).unwrap();
        for (prefix, cover) in [
            ("chapter/", "chapter/cover.jpg"),
            ("chapter/sub/", "chapter/sub/cover.jpg"),
            ("other/", "other/cover.jpg"),
        ] {
            db.save(
                &format!(
                    "{}|autocrop:v4:rtl",
                    crate::grid_item::zipdir_cache_key(prefix)
                ),
                1,
                2,
                3,
                4,
                None,
                b"not decoded",
                &CoverIdentity::Page(crate::adjustment_db::zip_entry_key(&path, cover)),
                automatic(),
            )
            .unwrap();
        }
        let completion = run_invalidation(request(
            &cache,
            &crate::adjustment_db::zip_entry_key(&path, "chapter/sub/body.jpg"),
            vec![zip_hint(&path)],
            vec![],
        ));
        assert!(completion.errors.is_empty(), "{:?}", completion.errors);
        assert_eq!(completion.affected_slots.len(), 2);
        assert!(
            db.load_one("zipdir:chapter/|autocrop:v4:rtl")
                .unwrap()
                .is_none()
        );
        assert!(
            db.load_one("zipdir:chapter/sub/|autocrop:v4:rtl")
                .unwrap()
                .is_none()
        );
        assert!(
            db.load_one("zipdir:other/|autocrop:v4:rtl")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn cover_rotation_pending_pdf_pin_and_direct_folder_sort_share_worker_selectors() {
        let temp = tempfile::tempdir().unwrap();
        let pdf = temp.path().join("book.pdf");
        std::fs::write(&pdf, b"metadata only, not a PDF").unwrap();
        let pins = Arc::new(
            crate::folder_thumb_pins::FolderThumbPinDb::open_at(&temp.path().join("pins.db"))
                .unwrap(),
        );
        pins.set(
            &pdf,
            &crate::folder_thumb_pins::FolderPinSource::PdfPage {
                pdf_rel: String::new(),
                page: 3,
            },
        )
        .unwrap();
        let mut hint = SourceHint::new(BookCoverId::Pdf(pdf.clone()));
        hint.selection = Some(SelectionHint {
            request: crate::thumb_loader::LoadRequest {
                path: pdf.clone(),
                auto_cover_crop: Some(crate::cover_auto_crop::AutoCropIdentity::new(
                    crate::settings::ReadingDirection::Rtl,
                )),
                ..Default::default()
            },
            pin_db: Some(pins),
        });
        assert_eq!(
            selected_representative(&hint),
            Some(crate::edit_source::page_key_for_pdf(&pdf, 3))
        );
        let folder = temp.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        for name in ["2.jpg", "10.jpg"] {
            std::fs::write(folder.join(name), b"not an image").unwrap();
        }
        let policy = crate::file_type_visibility::FileTypeVisibility::default();
        assert_eq!(
            crate::thumb_loader::direct_folder_representative_key(
                &folder,
                crate::settings::SortOrder::Numeric,
                &policy
            ),
            Some(crate::adjustment_db::normalize_path(&folder.join("2.jpg")))
        );
        assert_eq!(
            crate::thumb_loader::direct_folder_representative_key(
                &folder,
                crate::settings::SortOrder::FileName,
                &policy
            ),
            // FileName uses the released natural FileNameSortKey, too.
            Some(crate::adjustment_db::normalize_path(&folder.join("2.jpg")))
        );
    }

    #[test]
    fn cover_rotation_keys_only_notifications_reuse_existing_typed_source_restoration() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let pdf = temp.path().join("book.pdf");
        std::fs::write(&pdf, b"source metadata only").unwrap();
        let cover = crate::edit_source::page_key_for_pdf(&pdf, 0);
        let db = crate::catalog::CatalogDb::open(&cache, temp.path()).unwrap();
        db.save(
            "pdfthumb:book.pdf|autocrop:v4:rtl",
            1,
            2,
            3,
            4,
            None,
            b"no decoding",
            &CoverIdentity::Page(cover.clone()),
            automatic(),
        )
        .unwrap();
        let changed = crate::edit_source::page_key_for_pdf(&pdf, 2);
        let completion = run_invalidation(request(&cache, &changed, vec![], vec![]));
        assert!(completion.errors.is_empty(), "{:?}", completion.errors);
        assert_eq!(completion.affected_slots.len(), 1);
        assert_eq!(completion.affected_slots[0].representative_page_key, cover);
        assert!(completion.pending_slots.is_empty());
        assert!(
            db.load_one("pdfthumb:book.pdf|autocrop:v4:rtl")
                .unwrap()
                .is_none()
        );
        assert!(source_hint_for_existing_page_key("c:/missing/book.pdf::page_2").is_none());
    }

    #[test]
    fn cover_rotation_late_memory_cleanup_uses_exact_slot_and_scope_not_key_substrings() {
        let scope = Path::new("c:/books");
        let representative = "c:/converted/generated.zip::cover.jpg";
        let slot = AutoCoverSlot {
            logical_book: BookCoverId::Archive(PathBuf::from("c:/books/book.7z")),
            representative_page_key: representative.into(),
        };
        let key = "archivethumb:book.7z|types:0123456789abcdef|autocrop:v4:rtl#pin:old";
        let other = "archivethumb:other.7z|autocrop:v4:rtl";
        let ordinary = "literal::page|autocrop:v4:rtl";
        let map = Arc::new(RwLock::new(HashMap::from([
            (key.into(), entry(representative)),
            (other.into(), entry(representative)),
            (ordinary.into(), entry(representative)),
        ])));
        assert_eq!(
            remove_invalidated_memory_rows(Path::new("c:/another"), &map, &[slot.clone()], &[]),
            0
        );
        assert_eq!(remove_invalidated_memory_rows(scope, &map, &[slot], &[]), 1);
        assert!(!map.read().unwrap().contains_key(key));
        assert!(map.read().unwrap().contains_key(other));
        assert!(map.read().unwrap().contains_key(ordinary));
    }

    #[test]
    fn cover_rotation_collection_inventory_decodes_covers_with_physical_source_drive() {
        let temp = tempfile::tempdir().unwrap();
        let source_parent = Path::new("c:/books");
        let storage = crate::catalog::collection_source_parent_identity(source_parent).unwrap();
        let db = crate::catalog::CatalogDb::open(temp.path(), &storage)
            .unwrap()
            .with_cover_source_folder(source_parent);
        let key = "zipthumb:c:/books/book.zip|autocrop:v4:ltr";
        let cover = "c:/books/book.zip::cover.jpg";
        db.save(
            key,
            1,
            2,
            3,
            4,
            None,
            b"no decoding",
            &CoverIdentity::Page(cover.into()),
            automatic(),
        )
        .unwrap();
        let completion = run_invalidation(request(
            temp.path(),
            "c:/books/book.zip::body.jpg",
            vec![zip_hint(Path::new("c:/books/book.zip"))],
            vec![],
        ));
        assert!(completion.errors.is_empty(), "{:?}", completion.errors);
        assert_eq!(completion.affected_slots.len(), 1);
        assert_eq!(completion.affected_slots[0].representative_page_key, cover);
        assert!(db.load_one(key).unwrap().is_none());
    }

    #[test]
    fn cover_rotation_zipdir_shares_outer_source_alias_without_invalidating_sibling_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("book.rar");
        let physical = temp.path().join("converted.zip");
        let mut outer = SourceHint::new(BookCoverId::Archive(source.clone()));
        outer.aliases.push(physical.clone());
        let mut selected = SourceHint::new(BookCoverId::Zip {
            path: source.clone(),
            prefix: Some("book/".into()),
        });
        let cover = crate::adjustment_db::zip_entry_key(&physical, "book/cover.png");
        selected.representative_page_keys.push(cover.clone());
        // Preserve a distinct selection, as in a retained owner. No pending
        // source selection or persisted row supplies this Loaded material.
        selected.selection = Some(SelectionHint {
            request: Default::default(),
            pin_db: None,
        });
        let mut sibling = SourceHint::new(BookCoverId::Zip {
            path: source,
            prefix: Some("other/".into()),
        });
        sibling
            .representative_page_keys
            .push(crate::adjustment_db::zip_entry_key(
                &physical,
                "other/cover.png",
            ));
        let target = selected.book.clone();
        let changed = crate::adjustment_db::zip_entry_key(&physical, "book/body.png");
        let done = run_invalidation(request(
            &temp.path().join("cache"),
            &changed,
            vec![outer, selected, sibling],
            vec![],
        ));
        assert!(done.errors.is_empty());
        assert_eq!(
            done.affected_slots,
            vec![AutoCoverSlot {
                logical_book: target,
                representative_page_key: cover
            }]
        );
        assert!(done.pending_slots.is_empty());
        assert!(done.deleted_keys.is_empty());
    }

    #[test]
    fn cover_rotation_session_stamps_survive_clone_and_only_completed_slots_advance() {
        let mut service = CoverRotationCache::default();
        let book = BookCoverId::Pdf(PathBuf::from("c:/books/book.pdf"));
        let slot = AutoCoverSlot {
            logical_book: book.clone(),
            representative_page_key: "c:/books/book.pdf::page_0".into(),
        };
        let pinned = AutoCoverSlot {
            logical_book: book,
            representative_page_key: "c:/books/book.pdf::page_3".into(),
        };
        let before = service.stamp(slot.clone());
        let retained = before.clone();
        let sibling = service.stamp(pinned);
        service
            .sender
            .send(InvalidationCompletion {
                affected_slots: vec![slot.clone()],
                ..Default::default()
            })
            .unwrap();
        assert!(service.is_current(&before));
        let completions = service.poll();
        assert_eq!(completions.len(), 1);
        assert!(!service.is_current(&before));
        assert!(!service.is_current(&retained));
        assert!(service.is_current(&sibling));
        assert!(service.is_current(&service.stamp(slot)));
        assert!(service.is_current(&CoverCacheStamp::Untracked));
    }
}
