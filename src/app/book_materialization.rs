//! Shared page-row projection with the existing visibility policy and duplicate removal.
//! Enumeration, cancellation, leases and adoption remain with the existing source owners.
//! Child-container positions are surface policy; only the direct page block is a book order.
use crate::{
    grid_item::GridItem,
    settings::{GridDisplayOrder, SortOrder},
    zip_loader::entry_basename,
    zip_tree::{ZipTree, representative_image, segment_is_archive},
};
use std::path::Path;

pub(crate) type PageRows = (Vec<GridItem>, Vec<Option<(i64, i64)>>);

/// `None` preserves the legacy child-first order (Remote normal list and rating history).
/// Other surfaces retain their existing configured category placement.
pub(crate) fn materialize_archive_level(
    tree: &ZipTree,
    prefix: &[String],
    sort: SortOrder,
    placement: Option<&GridDisplayOrder>,
) -> PageRows {
    let mut items: Vec<GridItem> = Vec::new();
    let mut metas: Vec<Option<(i64, i64)>> = Vec::new();
    let Some(node) = tree.node_at(prefix) else {
        return (items, metas);
    };

    // 現在階層までの prefix 文字列 ("chapters/" や ""、末尾 '/' 付き)。
    let prefix_str = if prefix.is_empty() {
        String::new()
    } else {
        format!("{}/", prefix.join("/"))
    };

    // 1) 子コンテナ (ZipDir)。セグメント名で sort。
    let mut dirs: Vec<_> = node
        .dirs
        .iter()
        .map(|(seg, child)| (seg, child, sort.name_key(seg)))
        .collect();
    dirs.sort_by(|(_, _, ak), (_, _, bk)| sort.compare_name_keys(ak, 0, bk, 0));
    for (seg, child, _) in dirs {
        // 代表画像。build() は画像エントリの経路上にしかノードを作らないので、
        // 任意の子ノードは部分木に必ず 1 枚以上画像を持つ → rep は実質的に常に Some。
        let rep = representative_image(child, sort);
        items.push(GridItem::ZipDir {
            zip_path: tree.zip_path.clone(),
            dir_prefix: format!("{prefix_str}{seg}/"),
            is_archive: segment_is_archive(seg),
            representative: rep.map(|e| e.entry_name.clone()),
        });
        // 代表画像の mtime/size を meta に載せる。これがないと enqueue 経路
        // (image_metas[i]==None を skip) が ZipDir のサムネ要求を出さず、セルが
        // 永久 Pending → 毎フレーム repaint になる (Phase 2 Codex P2)。size 列に
        // 代表画像サイズが出るのは許容 (date 列は details_created_time_path で None)。
        metas.push(rep.map(|e| (e.mtime, e.uncompressed_size as i64)));
    }

    // 2) 直下画像 (ZipImage)。basename + mtime で sort。
    let mut imgs: Vec<_> = node
        .images
        .iter()
        .map(|entry| {
            let key = sort.name_key(entry_basename(&entry.entry_name));
            (entry, key)
        })
        .collect();
    imgs.sort_by(|(a, ak), (b, bk)| sort.compare_name_keys(ak, a.mtime, bk, b.mtime));
    for (e, _) in imgs {
        items.push(GridItem::ZipImage {
            zip_path: tree.zip_path.clone(),
            entry_name: e.entry_name.clone(),
        });
        metas.push(Some((e.mtime, e.uncompressed_size as i64)));
    }

    if let Some(placement) = placement {
        crate::grid_item::arrange_grid_items(&mut items, &mut metas, placement, Some(sort));
    }
    (items, metas)
}

pub(crate) fn materialize_cached_document_rows(
    pdf_path: &Path,
    page_count: u32,
    mtime: i64,
    file_size: u64,
) -> (
    Vec<GridItem>,
    Vec<Option<(i64, i64)>>,
    std::collections::HashSet<String>,
) {
    let mut items: Vec<GridItem> = Vec::with_capacity(page_count as usize);
    let mut image_metas: Vec<Option<(i64, i64)>> = Vec::with_capacity(page_count as usize);
    let mut existing_keys: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(page_count as usize);
    for i in 0..page_count {
        items.push(document_page_item(pdf_path, i));
        image_metas.push(Some((mtime, file_size as i64)));
        existing_keys.insert(crate::grid_item::pdf_page_cache_key(i));
    }
    (items, image_metas, existing_keys)
}

pub(crate) fn materialize_document_rows(
    pdf_path: &Path,
    pages: &[crate::pdf_loader::PdfPageEntry],
) -> (
    Vec<GridItem>,
    Vec<Option<(i64, i64)>>,
    std::collections::HashSet<String>,
) {
    let mut items = Vec::with_capacity(pages.len());
    let mut image_metas = Vec::with_capacity(pages.len());
    let mut existing_keys = std::collections::HashSet::with_capacity(pages.len());
    for page in pages {
        existing_keys.insert(crate::grid_item::pdf_page_cache_key(page.page_num));
        items.push(document_page_item(pdf_path, page.page_num));
        image_metas.push(Some((page.mtime, page.file_size as i64)));
    }
    (items, image_metas, existing_keys)
}

use super::folder_scan::{
    FolderRatingOrder, MaterializedFolderListing, ScanMediaKind, ScannedDir,
    filter_convertible_archive_duplicates_with_sort_metadata,
    filter_epub_pdf_duplicates_with_sort_metadata, filter_image_ext_duplicates,
    filter_video_image_duplicates, filter_virtual_folder_duplicates_with_sort_metadata,
    project_scanned_directory,
};

pub(crate) fn materialize_local_folder_listing(
    path: &std::path::Path,
    scan: ScannedDir,
    settings: &crate::settings::Settings,
) -> MaterializedFolderListing {
    materialize_local_folder_listing_with_order(
        path,
        scan,
        settings,
        crate::rating_sort::ListingOrderRequest::from_settings(settings),
        |keys| {
            crate::rating_db::RatingDb::open_readonly(crate::rating_db::RatingDb::db_path())
                .map_err(|error| error.to_string())?
                .get_many_complete(keys, 0)
                .map_err(|error| error.to_string())
        },
    )
}

pub(crate) fn materialize_local_folder_listing_with_order(
    path: &std::path::Path,
    scan: ScannedDir,
    settings: &crate::settings::Settings,
    request: crate::rating_sort::ListingOrderRequest,
    read_rating: impl FnOnce(&[String]) -> Result<crate::rating_db::CompleteRatingFacts, String>,
) -> MaterializedFolderListing {
    let mut scan = scan;
    project_scanned_directory(&mut scan, &settings.file_type_visibility);
    let mut auxiliary_media = scan.auxiliary_media;
    let mut omitted = scan.omitted;
    let mut folders = Vec::with_capacity(scan.folders.len());
    let mut metas = Vec::with_capacity(scan.folders.len());
    let mut sort_metas = Vec::with_capacity(scan.folders.len() + scan.all_media.len());
    for entry in scan.folders {
        folders.push(entry.item);
        metas.push(entry.display_meta);
        sort_metas.push(entry.sort_meta);
    }
    let mut all_media = scan.all_media;
    let compiled = crate::books::is_direct_book_folder(&settings.books_root_path(), path);
    if compiled {
        folders.clear();
        metas.clear();
        sort_metas.clear();
        all_media.retain(|entry| entry.kind == ScanMediaKind::Image);
    }
    let folder_count = folders.len();
    let media_count = all_media.len();

    let sort_started = std::time::Instant::now();
    let folder_sort = if compiled {
        crate::settings::SortOrder::Numeric
    } else {
        match request {
            crate::rating_sort::ListingOrderRequest::Standard(sort) => sort,
            crate::rating_sort::ListingOrderRequest::Rating(_) => {
                crate::settings::SortOrder::FileName
            }
        }
    };
    let media_sort = super::folder_media_sort_order(
        folder_sort,
        compiled,
        folders.is_empty(),
        all_media
            .iter()
            .all(|entry| entry.kind == ScanMediaKind::Image),
        settings.auto_fullscreen_image_folders_enabled(),
    );
    crate::grid_item::sort_folder_block_with_metadata(
        &mut folders,
        &mut metas,
        &mut sort_metas,
        folder_sort,
    );
    let mut keyed_media = all_media
        .into_iter()
        .map(|entry| {
            let name = entry
                .path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            let key = media_sort.name_key(name);
            (entry, key)
        })
        .collect::<Vec<_>>();
    keyed_media.sort_by(|(left, left_key), (right, right_key)| {
        media_sort.compare_listing_keys(left_key, left.sort_meta, right_key, right.sort_meta)
    });
    let mut all_media = keyed_media
        .into_iter()
        .map(|(entry, _)| entry)
        .collect::<Vec<_>>();
    let mut keyed_auxiliary = auxiliary_media
        .entries
        .into_iter()
        .map(|entry| {
            let name = entry
                .path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            let key = media_sort.name_key(name);
            (entry, key)
        })
        .collect::<Vec<_>>();
    keyed_auxiliary.sort_by(|(left, left_key), (right, right_key)| {
        media_sort.compare_listing_keys(left_key, left.sort_meta, right_key, right.sort_meta)
    });
    auxiliary_media.entries = keyed_auxiliary
        .into_iter()
        .map(|(entry, _)| entry)
        .collect();
    let sort_ms = sort_started.elapsed().as_secs_f64() * 1000.0;

    let duplicate_started = std::time::Instant::now();
    let mut same_name = 0usize;
    if settings.skip_zip_if_folder_exists {
        same_name = same_name.saturating_add(filter_virtual_folder_duplicates_with_sort_metadata(
            &mut folders,
            &mut metas,
            &mut sort_metas,
        ));
    }
    if settings.skip_archive_if_zip_exists {
        same_name =
            same_name.saturating_add(filter_convertible_archive_duplicates_with_sort_metadata(
                &mut folders,
                &mut metas,
                &mut sort_metas,
            ));
    }
    if settings.skip_epub_if_pdf_exists {
        same_name = same_name.saturating_add(filter_epub_pdf_duplicates_with_sort_metadata(
            &mut folders,
            &mut metas,
            &mut sort_metas,
        ));
    }
    let video_thumb_overrides =
        if settings.skip_image_if_video_exists && settings.video_thumb_use_sidecar_image {
            let surviving_sources = all_media
                .iter()
                .filter(|entry| matches!(entry.kind, ScanMediaKind::Video | ScanMediaKind::Audio))
                .map(|entry| &entry.path)
                .collect::<std::collections::HashSet<_>>();
            auxiliary_media
                .sidecars()
                .into_iter()
                .filter(|(source, _)| surviving_sources.contains(source))
                .collect()
        } else {
            Vec::new()
        };
    if settings.skip_image_if_video_exists {
        let filtered = filter_video_image_duplicates(&mut all_media, false);
        same_name = same_name.saturating_add(filtered.omitted);
    }
    if settings.skip_duplicate_images {
        same_name = same_name.saturating_add(filter_image_ext_duplicates(
            &mut all_media,
            &settings.image_ext_priority,
        ));
    }
    omitted.same_name = same_name;
    let duplicate_filter_ms = duplicate_started.elapsed().as_secs_f64() * 1000.0;

    let mut items = folders;
    for entry in all_media {
        items.push(match entry.kind {
            ScanMediaKind::Image => GridItem::Image(entry.path),
            ScanMediaKind::Video => GridItem::Video(entry.path),
            ScanMediaKind::Audio => GridItem::Audio(entry.path),
        });
        metas.push(Some((entry.mtime, entry.file_size)));
        sort_metas.push(entry.sort_meta);
    }
    // Candidate exclusion is complete. Rating facts are read once here, before
    // the final arrangement and before any index (including video indices) is derived.
    let rating_order = if let crate::rating_sort::ListingOrderRequest::Rating(spec) = request {
        if compiled || media_sort != folder_sort {
            crate::grid_item::arrange_grid_items_with_sort_metadata(
                &mut items,
                &mut metas,
                &mut sort_metas,
                &settings.grid_display_order,
                Some(media_sort),
            );
            FolderRatingOrder::Standard
        } else {
            let item_keys = items
                .iter()
                .map(crate::rating_sort::physical_item_key)
                .collect::<Vec<_>>();
            let requested = item_keys
                .iter()
                .filter_map(Clone::clone)
                .collect::<Vec<_>>();
            let read_started = std::time::Instant::now();
            let read_result = read_rating(&requested);
            let read_ms = read_started.elapsed().as_secs_f64() * 1000.0;
            match read_result {
                Ok(facts) => {
                    let mut rating_keys = item_keys
                        .iter()
                        .map(|key| match key {
                            Some(key) => facts.key_for_requested(key),
                            None => crate::rating_sort::RatingSortKey::Unsupported,
                        })
                        .collect::<Vec<_>>();
                    crate::grid_item::arrange_grid_items_with_rating_keys(
                        &mut items,
                        &mut metas,
                        &mut sort_metas,
                        &mut rating_keys,
                        &settings.grid_display_order,
                        spec,
                    );
                    let values = facts
                        .iter_requested_ratings()
                        .map(|(key, stars)| (key.to_owned(), stars))
                        .collect::<std::collections::HashMap<_, _>>();
                    let cache = items
                        .iter()
                        .enumerate()
                        .filter_map(|(idx, item)| {
                            crate::rating_sort::physical_item_key(item)
                                .map(|key| (idx, values[&key]))
                        })
                        .collect();
                    FolderRatingOrder::Prepared {
                        cache,
                        write_generation: facts.write_generation(),
                        requested_count: requested.len(),
                        read_ms,
                    }
                }
                Err(_) => {
                    crate::grid_item::arrange_grid_items_with_sort_metadata(
                        &mut items,
                        &mut metas,
                        &mut sort_metas,
                        &settings.grid_display_order,
                        Some(crate::settings::SortOrder::FileName),
                    );
                    FolderRatingOrder::Failed
                }
            }
        }
    } else {
        crate::grid_item::arrange_grid_items_with_sort_metadata(
            &mut items,
            &mut metas,
            &mut sort_metas,
            &settings.grid_display_order,
            Some(media_sort),
        );
        FolderRatingOrder::Standard
    };

    MaterializedFolderListing {
        items,
        metas,
        sort_metas,
        omitted,
        video_thumb_overrides,
        folder_count,
        media_count,
        folder_sort,
        sort_ms,
        duplicate_filter_ms,
        rating_order,
    }
}

/// A source's accepted semantic surface, independent of its current anchor or path spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExtractedSurface {
    Search(super::top_level_grid_view::TopLevelSearchView),
    Rating,
    SmartFolder,
    Collection,
    Snapshot,
    SubfolderExpansion,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BookScope {
    Archive {
        logical_root: std::path::PathBuf,
        prefix: Vec<String>,
    },
    Document(std::path::PathBuf),
    ImageFolder(std::path::PathBuf),
    CompiledFolder(std::path::PathBuf),
}

/// One accepted classification travels with items, snapshots and viewer-context bundles.
/// Membership/order and raw manifests retain their existing owners; no duplicate row copy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ProjectionRecipe {
    Book(BookScope),
    #[default]
    OrdinaryList,
    ExtractedResult(ExtractedSurface),
}

impl ProjectionRecipe {
    pub(crate) fn for_physical(
        settings: &crate::settings::Settings,
        path: &Path,
        logical: &Path,
        items: &[GridItem],
    ) -> Self {
        if !super::physical_page_order_locked(settings, path, items)
            || items
                .iter()
                .any(|item| matches!(item, GridItem::Video(_) | GridItem::Audio(_)))
        {
            return Self::OrdinaryList;
        }
        let scope = if crate::folder_tree::is_paged_document_path(logical) {
            BookScope::Document(logical.to_path_buf())
        } else if crate::folder_tree::is_open_as_container(path) {
            BookScope::Archive {
                logical_root: logical.to_path_buf(),
                prefix: Vec::new(),
            }
        } else if crate::books::is_direct_book_folder(&settings.books_root_path(), path) {
            BookScope::CompiledFolder(path.to_path_buf())
        } else {
            BookScope::ImageFolder(path.to_path_buf())
        };
        Self::Book(scope)
    }

    pub(crate) fn extracted_surface(&self) -> Option<ExtractedSurface> {
        match self {
            Self::ExtractedResult(surface) => Some(*surface),
            _ => None,
        }
    }
}

pub(crate) fn document_page_item(logical_path: &Path, page_num: u32) -> GridItem {
    GridItem::PdfPage {
        pdf_path: logical_path.to_path_buf(),
        page_num,
        content_type: None,
    }
}

impl super::App {
    pub(crate) fn extracted_result_surface(&self) -> Option<ExtractedSurface> {
        self.projection_recipe.extracted_surface()
    }

    pub(crate) fn adopt_archive_recipe(&mut self) {
        if let Some(nav) = self.zip_nav.as_ref() {
            self.projection_recipe = ProjectionRecipe::Book(BookScope::Archive {
                logical_root: self
                    .archive_source_override
                    .as_deref()
                    .unwrap_or(&nav.tree.zip_path)
                    .to_path_buf(),
                prefix: nav.current().to_vec(),
            });
        }
    }

    /// New result-list viewing uses a session snapshot of the global direction. A fork of
    /// an already open viewer retains its presentation through the existing context bundle.
    pub(crate) fn begin_extracted_result_view(&mut self) {
        self.spread_mode = crate::settings::SpreadMode::Single;
        self.reading_direction = self.settings.default_reading_direction;
        // Detach the preceding book's session without issuing a durable Clear.
        self.spread_shift = Default::default();
        self.book_composition = Default::default();
    }
}

/// Existing direction/cover-specific shortcuts select a view kind on extracted lists.
/// Their binding bits never become result-session settings.
pub(crate) fn extracted_view_mode(
    mode: crate::settings::SpreadMode,
    direction: crate::settings::ReadingDirection,
) -> crate::settings::SpreadMode {
    use crate::settings::SpreadMode;
    if mode.is_spread() {
        SpreadMode::Ltr.with_reading_direction(direction)
    } else if mode.is_split() {
        SpreadMode::SplitLtr.with_reading_direction(direction)
    } else {
        SpreadMode::Single
    }
}

pub(crate) fn resolve_archive_target_index(
    tree: &ZipTree,
    target: &crate::book_bookmarks::ResolvedArchiveBookmarkTarget,
    placement: &GridDisplayOrder,
) -> Option<usize> {
    let prefix = target
        .effective_prefix
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let (items, _) = materialize_archive_level(
        tree,
        &prefix,
        super::BOOK_READING_PAGE_ORDER,
        Some(placement),
    );
    // Preserve the bookmark resolver's existing case/separator equivalence, including
    // the first materialized match when an archive contains case-distinct aliases.
    let wanted = crate::book_bookmarks::normalize_page_path(&target.entry_name);
    items.iter().position(|item| {
        matches!(item, GridItem::ZipImage { entry_name, .. }
        if crate::book_bookmarks::normalize_page_path(entry_name) == wanted)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{GridItemDisplayKind, ReadingDirection, SpreadMode};
    use std::path::PathBuf;

    fn archive() -> ZipTree {
        ZipTree::build(
            PathBuf::from("C:/converted/book.zip"),
            [
                "wrapper/20.jpg",
                "wrapper/02.jpg",
                "wrapper/child/01.jpg",
                "wrapper/nested.zip/03.jpg",
            ]
            .into_iter()
            .enumerate()
            .map(|(i, name)| crate::zip_loader::ZipImageEntry {
                entry_name: name.to_owned(),
                mtime: (10 - i) as i64,
                uncompressed_size: i as u64 + 100,
                container_ancestry: Vec::new(),
            })
            .collect(),
        )
    }

    #[test]
    fn g1_page_block_matches_while_surface_child_positions_remain_distinct() {
        let tree = archive();
        let scope = vec!["wrapper".to_owned()];
        let mut placement = GridDisplayOrder::default();
        placement.assign(GridItemDisplayKind::Image, 0);
        placement.assign(GridItemDisplayKind::Archive, 1);
        let (legacy, legacy_metas) =
            materialize_archive_level(&tree, &scope, super::super::BOOK_READING_PAGE_ORDER, None);
        let (pc, pc_metas) = materialize_archive_level(
            &tree,
            &scope,
            super::super::BOOK_READING_PAGE_ORDER,
            Some(&placement),
        );
        assert!(matches!(legacy[0], GridItem::ZipDir { .. }));
        assert!(matches!(pc[0], GridItem::ZipImage { .. }));
        let pages = |items: Vec<GridItem>, metas: Vec<Option<(i64, i64)>>| {
            items
                .into_iter()
                .zip(metas)
                .filter(|(item, _)| matches!(item, GridItem::ZipImage { .. }))
                .collect::<Vec<_>>()
        };
        let expected = pages(legacy, legacy_metas);
        assert_eq!(pages(pc, pc_metas), expected);
        assert!(
            matches!(&expected[0].0, GridItem::ZipImage { entry_name, .. } if entry_name == "wrapper/02.jpg")
        );
        assert_eq!(expected[0].1, Some((9, 101)));
        let mut nav = crate::zip_tree::ZipNavState::new(std::sync::Arc::new(tree));
        assert_eq!(nav.current(), scope);
        nav.enter("wrapper/nested.zip/");
        let (nested, metas) = nav.materialize_current(super::super::BOOK_READING_PAGE_ORDER);
        assert_eq!(nested.len(), metas.len());
        assert!(
            matches!(&nested[0], GridItem::ZipImage { entry_name, .. } if entry_name == "wrapper/nested.zip/03.jpg")
        );
    }

    #[test]
    fn g1_bookmark_index_preserves_legacy_case_alias_selection_after_sort() {
        let tree = ZipTree::build(
            PathBuf::from("C:/book.zip"),
            ["page.jpg", "PAGE.JPG"]
                .into_iter()
                .map(|name| crate::zip_loader::ZipImageEntry {
                    entry_name: name.to_owned(),
                    mtime: 1,
                    uncompressed_size: 10,
                    container_ancestry: Vec::new(),
                })
                .collect(),
        );
        let target =
            crate::book_bookmarks::resolve_archive_bookmark_target(&tree, "PAGE.JPG").unwrap();
        assert_eq!(target.entry_name, "page.jpg"); // Original enumeration lookup.
        let placement = GridDisplayOrder::default();
        let (rows, _) = materialize_archive_level(
            &tree,
            &[],
            super::super::BOOK_READING_PAGE_ORDER,
            Some(&placement),
        );
        assert!(
            matches!(&rows[0], GridItem::ZipImage { entry_name, .. } if entry_name == "PAGE.JPG")
        );
        // Released lookup chooses the first normalized match in the displayed order.
        assert_eq!(
            resolve_archive_target_index(&tree, &target, &placement),
            Some(0)
        );
    }

    #[test]
    fn g1_document_cached_and_completed_rows_keep_logical_epub_identity() {
        let logical = Path::new("C:/source/book.epub");
        let pages = (0..4)
            .map(|page_num| crate::pdf_loader::PdfPageEntry {
                page_num,
                mtime: 17,
                file_size: 41,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            materialize_cached_document_rows(logical, 4, 17, 41),
            materialize_document_rows(logical, &pages)
        );
        for (i, item) in materialize_document_rows(logical, &pages)
            .0
            .iter()
            .enumerate()
        {
            assert!(
                matches!(item, GridItem::PdfPage { pdf_path, page_num, .. } if pdf_path == logical && *page_num == i as u32)
            );
        }
    }

    #[test]
    fn g1_physical_classification_uses_accepted_policy_rows_and_keeps_mixed_ordinary() {
        let mut settings = crate::settings::Settings::default();
        let folder = Path::new("C:/images");
        let mut rows = vec![GridItem::Image(folder.join("02.jpg"))];
        assert_eq!(
            ProjectionRecipe::for_physical(&settings, folder, folder, &rows),
            ProjectionRecipe::OrdinaryList
        );
        settings.auto_fullscreen_zip_pdf = true;
        settings.auto_fullscreen_image_folders = true;
        assert_eq!(
            ProjectionRecipe::for_physical(&settings, folder, folder, &rows),
            ProjectionRecipe::Book(BookScope::ImageFolder(folder.to_owned()))
        );
        rows.push(GridItem::Video(folder.join("clip.mp4")));
        assert_eq!(
            ProjectionRecipe::for_physical(&settings, folder, folder, &rows),
            ProjectionRecipe::OrdinaryList
        );
        rows.pop(); // a policy-projected result can qualify, without reintroducing the excluded video
        assert!(matches!(
            ProjectionRecipe::for_physical(&settings, folder, folder, &rows),
            ProjectionRecipe::Book(_)
        ));
        let compiled = settings.books_root_path().join("compiled");
        settings.auto_fullscreen_zip_pdf = false;
        assert_eq!(
            ProjectionRecipe::for_physical(&settings, &compiled, &compiled, &rows),
            ProjectionRecipe::Book(BookScope::CompiledFolder(compiled))
        );
    }

    #[test]
    fn g1_all_extracted_surfaces_remain_extracted_even_for_one_book_or_mixed_rows() {
        for surface in [
            ExtractedSurface::Search(super::super::top_level_grid_view::TopLevelSearchView::Global),
            ExtractedSurface::Search(super::super::top_level_grid_view::TopLevelSearchView::Tag),
            ExtractedSurface::Search(
                super::super::top_level_grid_view::TopLevelSearchView::Favorite,
            ),
            ExtractedSurface::Rating,
            ExtractedSurface::SmartFolder,
            ExtractedSurface::Collection,
            ExtractedSurface::Snapshot,
            ExtractedSurface::SubfolderExpansion,
        ] {
            // Accepted provenance wins over the rows: one physical book and a mixed
            // cross-book result both remain A2, with aligned metadata and initial Single.
            let mut app = crate::app::setup_app_for_test();
            app.settings.default_spread_mode = SpreadMode::RtlCover;
            app.settings.default_reading_direction = ReadingDirection::Rtl;
            for rows in [
                vec![GridItem::Image(PathBuf::from("C:/book/01.jpg"))],
                vec![
                    GridItem::Image(PathBuf::from("C:/book/01.jpg")),
                    GridItem::Video(PathBuf::from("C:/other/video.mp4")),
                    document_page_item(Path::new("C:/other.pdf"), 8),
                ],
            ] {
                app.spread_mode = SpreadMode::LtrCover;
                let metas = vec![Some((7, 11)); rows.len()];
                app.install_prepared_aggregate_items(
                    ProjectionRecipe::ExtractedResult(surface),
                    rows.clone(),
                    metas.clone(),
                    Default::default(),
                );
                let defaults = super::super::SpreadRestoreDefaults::for_book(&app.settings);
                app.apply_spread_for_key_with_fallback(Path::new("C:/book"), None, defaults)
                    .unwrap();
                assert_eq!(app.extracted_result_surface(), Some(surface));
                assert_eq!(app.items, rows);
                assert_eq!(app.image_metas, metas);
                assert_eq!(app.spread_mode, SpreadMode::Single);
                assert_eq!(app.reading_direction, ReadingDirection::Rtl);
            }
        }
        for direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
            for mode in SpreadMode::all() {
                let actual = extracted_view_mode(*mode, direction);
                assert!(!actual.has_cover());
                assert_eq!(actual.reading_direction().unwrap_or(direction), direction);
                assert_eq!(actual.is_spread(), mode.is_spread());
                assert_eq!(actual.is_split(), mode.is_split());
            }
        }
    }
}
