//! Read-only base grid listings. Enumeration and composition use the grid's
//! pure adapters; the agent never adopts UI state or runs metadata migration.
use super::*;
use crate::app::book_materialization as materialize;
use crate::book_composition::{CompositionProjectionOutcome, CompositionSnapshot};
use crate::grid_item::GridItem;
use crate::settings::Settings;

const AGENT_NESTED_LIMITS: crate::zip_loader::NestedZipLimits =
    crate::zip_loader::NestedZipLimits {
        max_archive_bytes: 64 * 1024 * 1024,
        max_total_bytes: 256 * 1024 * 1024,
    };

impl Handlers {
    pub(super) fn list_folder(
        &self,
        path: String,
        subresource: Option<String>,
        limit: Option<u32>,
        cursor: Option<String>,
        ctx: &ReadContext,
    ) -> Result<Payload, AgentError> {
        ctx.checkpoint()?;
        folder_limit(limit)?;
        let path = input_path::validate(&path)?;
        let prefix = folder_prefix(subresource.as_deref())?;
        let mut settings = crate::settings_db::with_db_result(|db| db.load_into_settings())
            .map_err(|_| internal())?;
        ctx.checkpoint()?;
        let io = crate::io_semaphore::GlobalIoSemaphore::process_shared(
            settings.indexer_speed_profile.io_permits(),
        );
        let _io = io
            .acquire_cancellable(crate::io_semaphore::IoPriority::Normal, &ctx.cancel)
            .ok_or_else(cancelled)?;
        overlay_saved_favorite_view(&mut settings, path)?;
        ctx.checkpoint()?;
        let metadata = std::fs::metadata(path).map_err(|_| internal())?;
        let (items, applied) = if metadata.is_dir() {
            if subresource.is_some() {
                return Err(bad_folder());
            }
            (physical_folder(path, &settings, ctx)?, false)
        } else {
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !crate::folder_tree::is_zip_extension(&extension) && extension != "pdf" {
                return Err(error(
                    ErrorCode::BadRequest,
                    "この形式の中は一覧できません。mIV で開いてください",
                ));
            }
            let service = self
                .readers
                .book_composition()
                .ok_or_else(composition_loading)?;
            // snapshot remains readable after close_admission during shutdown.
            let snapshot = service.snapshot();
            if snapshot.status.is_err() {
                return Err(composition_loading());
            }
            if extension == "pdf" {
                if subresource.is_some() {
                    return Err(bad_folder());
                }
                document_folder(path, &metadata, &settings, &snapshot, ctx)?
            } else {
                archive_folder(path, &prefix, &settings, &snapshot, ctx)?
            }
        };
        ctx.checkpoint()?;
        let args = serde_json::json!([
            physical_identity(path),
            prefix
                .iter()
                .map(|segment| segment.to_lowercase())
                .collect::<Vec<_>>(),
        ]);
        let page = folder_page(items, applied, limit, cursor.as_deref(), &args)?;
        ctx.checkpoint()?;
        Ok(Payload::Folder(page))
    }
}

fn bad_folder() -> AgentError {
    error(ErrorCode::BadRequest, "フォルダの指定が正しくありません")
}

fn physical_identity(path: &Path) -> String {
    input_path::key(path)
}

fn composition_loading() -> AgentError {
    error(ErrorCode::NotReady, "本の構成を読み込み中です")
}

fn folder_prefix(subresource: Option<&str>) -> Result<Vec<String>, AgentError> {
    let Some(prefix) = subresource else {
        return Ok(vec![]);
    };
    let prefix = prefix.strip_suffix('/').ok_or_else(bad_folder)?;
    crate::book_scope::validate_relative_reference(prefix).map_err(|_| bad_folder())?;
    Ok(prefix
        .replace('\\', "/")
        .split('/')
        .map(str::to_owned)
        .collect())
}

fn overlay_saved_favorite_view(settings: &mut Settings, path: &Path) -> Result<(), AgentError> {
    if settings.remember_favorite_view_state
        && crate::adjustment_db::AdjustmentDb::db_path()
            .try_exists()
            .map_err(|_| internal())?
    {
        let db = crate::adjustment_db::AdjustmentDb::open_readonly().map_err(|_| internal())?;
        let states = db.load_all_favorite_view_states();
        let id = crate::final_composite::active_favorite_default_id_for_path(
            path,
            &settings.favorites,
            None,
            |id| states.contains_key(&id),
        );
        if let Some(id) = id {
            settings.apply_favorite_view_overlay(id, &states[&id]);
        }
    }
    Ok(())
}

fn physical_folder(
    path: &Path,
    settings: &Settings,
    ctx: &ReadContext,
) -> Result<Vec<FolderItem>, AgentError> {
    let scan = crate::app::folder_scan::scan_directory_with_settings_cancel(
        path,
        settings,
        Some(&ctx.cancel),
    );
    ctx.checkpoint()?;
    let scan = scan.map_err(|_| internal())?;
    let known_metadata = scan
        .precise_stamps
        .iter()
        .filter_map(|(key, stamp)| stamp.is_some().then_some(*key))
        .collect::<std::collections::HashSet<_>>();
    let rows = materialize::materialize_local_folder_listing_with_order(
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
    );
    ctx.checkpoint()?;
    let metas = rows
        .items
        .iter()
        .zip(rows.metas)
        .map(|(item, meta)| {
            item.drag_source_path()
                .filter(|path| {
                    known_metadata.contains(&crate::app::folder_scan::folder_scan_path_key(path))
                })
                .and(meta)
        })
        .collect();
    folder_items(rows.items, metas)
}

fn archive_folder(
    path: &Path,
    requested: &[String],
    settings: &Settings,
    snapshot: &CompositionSnapshot,
    ctx: &ReadContext,
) -> Result<(Vec<FolderItem>, bool), AgentError> {
    let enumeration =
        crate::zip_loader::enumerate_image_entries_detailed_readonly_with_cancel_and_limits(
            path,
            Some(&ctx.cancel),
            Some(AGENT_NESTED_LIMITS),
        );
    ctx.checkpoint()?;
    let enumeration = enumeration.map_err(|cause| {
        if crate::zip_loader::is_nested_zip_limit_error(&cause) {
            error(ErrorCode::BadRequest, "大きすぎる入れ子の ZIP があります")
        } else {
            internal()
        }
    })?;
    let prepared = crate::zip_loader::PreparedZipEnumeration::prepare(
        path.to_path_buf(),
        enumeration,
        settings.file_type_visibility.clone(),
    );
    ctx.checkpoint()?;
    let tree = &prepared.tree;
    // Validate against the source tree: policy may hide all the rows of a real level.
    if tree.raw_source().node_at(requested).is_none() {
        return Err(bad_folder());
    }
    let raw_prefix = materialize::raw_archive_book_prefix(tree, requested);
    let display_prefix = tree.collapse_redundant(requested);
    let scope =
        crate::book_scope::BookScopeKey::archive(path, &raw_prefix).map_err(|_| bad_folder())?;
    let record = snapshot
        .rows
        .get(&scope.storage_key())
        .map(|row| &row.composition);
    // Grid uses the raw semantic level when composed, and the display collapse
    // otherwise. A visibility change must never switch the composition lookup key.
    let row_prefix = if record.is_some() {
        &raw_prefix
    } else {
        &display_prefix
    };
    let rows = materialize::materialize_archive_level(
        tree,
        row_prefix,
        crate::app::BOOK_READING_PAGE_ORDER,
        Some(&settings.grid_display_order),
    );
    ctx.checkpoint()?;
    let rows = materialize::project_archive_composition(
        tree,
        path,
        &raw_prefix,
        rows,
        crate::app::BOOK_READING_PAGE_ORDER,
        Some(&settings.grid_display_order),
        settings.file_type_visibility.clone(),
        snapshot.token_for(&scope),
        record,
    )
    .map_err(|_| internal())?;
    ctx.checkpoint()?;
    let applied = matches!(rows.composition, CompositionProjectionOutcome::Composed(_));
    Ok((folder_items(rows.items, rows.metas)?, applied))
}

fn document_folder(
    path: &Path,
    metadata: &std::fs::Metadata,
    settings: &Settings,
    snapshot: &CompositionSnapshot,
    ctx: &ReadContext,
) -> Result<(Vec<FolderItem>, bool), AgentError> {
    let mtime = crate::ui_helpers::mtime_secs(metadata);
    let file_size = metadata.len();
    let catalog = path
        .parent()
        .map(|parent| {
            crate::catalog::CatalogDb::open_existing_read_only(
                &crate::catalog::default_cache_dir(),
                parent,
            )
        })
        .transpose()
        .map_err(|_| internal())?
        .flatten();
    let persistent = catalog
        .as_ref()
        .zip(path.file_name().and_then(|name| name.to_str()))
        .map(|(catalog, filename)| {
            catalog.get_pdf_meta(
                filename,
                mtime,
                i64::try_from(file_size).unwrap_or(i64::MAX),
            )
        })
        .transpose()
        .map_err(|_| internal())?
        .flatten();
    drop(catalog);
    ctx.checkpoint()?;
    let page_count = match persistent {
        Some((count, _)) if count > 0 => count,
        _ => {
            let read = crate::pdf_loader::resolve_read_target(path).map_err(|_| internal())?;
            ctx.checkpoint()?;
            let pages = crate::pdf_loader::enumerate_pages_anonymous_with_read_target(
                path,
                &read,
                None,
                Some(ctx.cancel.clone()),
                crate::pdf_loader::EnumerateOptions::default(),
            );
            ctx.checkpoint()?;
            u32::try_from(pages.map_err(|_| internal())?.pages.len()).map_err(|_| internal())?
        }
    };
    let (items, metas, _) =
        materialize::materialize_cached_document_rows(path, page_count, mtime, file_size);
    let rows = if settings.file_type_visibility.allows_physical_file(path) {
        (items, metas)
    } else {
        (vec![], vec![])
    };
    let scope = crate::book_scope::BookScopeKey::document(path).map_err(|_| bad_folder())?;
    let record = snapshot
        .rows
        .get(&scope.storage_key())
        .map(|row| &row.composition);
    ctx.checkpoint()?;
    let rows = materialize::project_document_composition(
        path,
        page_count,
        rows,
        settings.file_type_visibility.clone(),
        snapshot.token_for(&scope),
        record,
    )
    .map_err(|_| internal())?;
    ctx.checkpoint()?;
    let applied = matches!(rows.composition, CompositionProjectionOutcome::Composed(_));
    Ok((folder_items(rows.items, rows.metas)?, applied))
}

fn folder_items(
    items: Vec<GridItem>,
    metas: Vec<Option<(i64, i64)>>,
) -> Result<Vec<FolderItem>, AgentError> {
    if items.len() != metas.len() {
        return Err(internal());
    }
    items
        .into_iter()
        .zip(metas)
        .map(|(item, meta)| {
            let name = item.name().into_owned();
            let (path, subresource, kind) = match item {
                GridItem::Folder(path) => (path, None, FolderKind::Folder),
                GridItem::Image(path) => (path, None, FolderKind::Image),
                GridItem::Video(path) => (path, None, FolderKind::Video),
                GridItem::Audio(path) => (path, None, FolderKind::Audio),
                GridItem::ZipFile(path) => (path, None, FolderKind::Zip),
                GridItem::PdfFile(path) => {
                    let kind = if path
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("epub"))
                    {
                        FolderKind::Epub
                    } else {
                        FolderKind::Pdf
                    };
                    (path, None, kind)
                }
                GridItem::ConvertibleArchive { path, .. } => (path, None, FolderKind::Archive),
                GridItem::ZipImage {
                    zip_path,
                    entry_name,
                } => (zip_path, Some(entry_name), FolderKind::ZipEntry),
                GridItem::ZipDir {
                    zip_path,
                    dir_prefix,
                    ..
                } => (zip_path, Some(dir_prefix), FolderKind::ZipFolder),
                GridItem::PdfPage {
                    pdf_path, page_num, ..
                } => (
                    pdf_path,
                    Some(format!("page_{page_num}")),
                    FolderKind::PdfPage,
                ),
                _ => return Err(internal()),
            };
            // Grid's ZipDir metadata is its representative image, and PDF metadata is
            // the whole document. Neither describes this virtual row's own size/date.
            let meta = if matches!(kind, FolderKind::ZipFolder | FolderKind::PdfPage) {
                None
            } else {
                meta
            };
            Ok(FolderItem {
                path: path.to_string_lossy().into_owned(),
                subresource,
                name,
                kind,
                size: if kind == FolderKind::Folder {
                    None
                } else {
                    meta.and_then(|(_, size)| u64::try_from(size).ok())
                },
                modified: meta.and_then(|(mtime, _)| mtime.checked_mul(1000)),
            })
        })
        .collect()
}

#[derive(Serialize, Deserialize)]
struct FolderCursor {
    version: u32,
    tool: String,
    args_digest: String,
    listing_digest: String,
    returned: usize,
}

fn folder_cursor(args_digest: &str, listing_digest: &str, returned: usize) -> String {
    serde_json::to_vec(&FolderCursor {
        version: 2,
        tool: "list_folder".into(),
        args_digest: args_digest.into(),
        listing_digest: listing_digest.into(),
        returned,
    })
    .expect("serializable folder cursor")
    .iter()
    .map(|byte| format!("{byte:02x}"))
    .collect()
}

fn folder_offset(
    cursor: Option<&str>,
    args: &str,
    listing: &str,
    total: usize,
) -> Result<usize, AgentError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let bad = || error(ErrorCode::BadRequest, "最初からやり直してください");
    if cursor.len() > 8192 || !cursor.len().is_multiple_of(2) || !cursor.is_ascii() {
        return Err(bad());
    }
    let bytes = (0..cursor.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&cursor[index..index + 2], 16).map_err(|_| bad()))
        .collect::<Result<Vec<_>, _>>()?;
    let cursor: FolderCursor = serde_json::from_slice(&bytes).map_err(|_| bad())?;
    if cursor.version != 2 || cursor.tool != "list_folder" || cursor.args_digest != args {
        return Err(bad());
    }
    if cursor.listing_digest != listing {
        return Err(error(
            ErrorCode::BadRequest,
            "一覧が変わりました。最初から取り直してください",
        ));
    }
    if cursor.returned > total {
        return Err(bad());
    }
    Ok(cursor.returned)
}

fn folder_page(
    items: Vec<FolderItem>,
    applied: bool,
    limit: Option<u32>,
    cursor: Option<&str>,
    args: &serde_json::Value,
) -> Result<FolderPage, AgentError> {
    let limit = folder_limit(limit)?;
    let args_digest = wire_digest(args).map_err(|_| internal())?;
    let identity = items
        .iter()
        .map(|item| (physical_identity(Path::new(&item.path)), &item.subresource))
        .collect::<Vec<_>>();
    let listing_digest = wire_digest(&identity).map_err(|_| internal())?;
    let offset = folder_offset(cursor, &args_digest, &listing_digest, items.len())?;
    let total = items.len();
    let items = items
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|item| {
            if mcp_content_chars(&item).map_err(|_| internal())? > MAX_ITEM_CHARS {
                PageRow::too_long(&item).map_err(|_| internal())
            } else {
                Ok(PageRow::Value(item))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut page = FolderPage {
        items,
        cursor: None,
        truncated: false,
        book_composition_applied: applied,
    };
    loop {
        page.truncated = offset + page.items.len() < total;
        page.cursor = (page.truncated && !page.items.is_empty())
            .then(|| folder_cursor(&args_digest, &listing_digest, offset + page.items.len()));
        if mcp_content_chars(&page).map_err(|_| internal())? <= MAX_PAGE_CHARS {
            return Ok(page);
        }
        page.items.pop();
        if page.items.is_empty() {
            return Err(internal());
        }
    }
}

fn folder_limit(limit: Option<u32>) -> Result<usize, AgentError> {
    match limit.unwrap_or(DEFAULT_FOLDER_LIMIT) {
        0 => Err(error(
            ErrorCode::BadRequest,
            "limit は1以上で指定してください",
        )),
        limit => Ok(limit.min(MAX_LIMIT) as usize),
    }
}

#[cfg(test)]
#[path = "folder_tests.rs"]
mod tests;
