//! Read-only base grid listings. Enumeration and composition use the grid's
//! pure adapters; the agent never adopts UI state or runs metadata migration.
use super::rating::RatingKey;
use super::*;
use crate::app::book_materialization as materialize;
use crate::book_composition::{CompositionProjectionOutcome, CompositionSnapshot};
use crate::grid_item::GridItem;
use crate::settings::Settings;
use std::collections::HashMap;
use std::path::PathBuf;

struct FolderRow {
    item: FolderItem,
    rating_key: Option<RatingKey>,
}

#[derive(Default, Serialize)]
struct FolderFilters {
    min_stars: Option<u8>,
    max_stars: Option<u8>,
    kinds: Option<Vec<FolderKind>>,
    name_contains: Option<String>,
    extensions: Option<Vec<String>>,
    tagged: Option<bool>,
}

impl FolderFilters {
    fn new(
        min_stars: Option<u8>,
        max_stars: Option<u8>,
        mut kinds: Option<Vec<FolderKind>>,
    ) -> Result<Self, AgentError> {
        if min_stars.is_some_and(|stars| stars > 5)
            || max_stars.is_some_and(|stars| stars > 5)
            || min_stars.zip(max_stars).is_some_and(|(min, max)| min > max)
        {
            return Err(error(
                ErrorCode::BadRequest,
                "★の範囲は0～5で指定してください",
            ));
        }
        if let Some(kinds) = &mut kinds {
            kinds.sort_unstable();
            if kinds.is_empty() || kinds.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(error(
                    ErrorCode::BadRequest,
                    "種類は重複せず1つ以上指定してください",
                ));
            }
        }
        Ok(Self {
            min_stars,
            max_stars,
            kinds,
            ..Self::default()
        })
    }

    fn with_extra(mut self, options: &FolderOptions) -> Result<Self, AgentError> {
        if options
            .name_contains
            .as_ref()
            .is_some_and(|name| !(1..=200).contains(&name.chars().count()))
        {
            return Err(error(
                ErrorCode::BadRequest,
                "name_contains は1～200文字で指定してください",
            ));
        }
        self.name_contains = options.name_contains.clone();
        self.tagged = options.tagged;
        if let Some(values) = &options.extensions {
            let mut values = values
                .iter()
                .map(|value| value.to_lowercase())
                .collect::<Vec<_>>();
            values.sort();
            if !(1..=20).contains(&values.len())
                || values
                    .iter()
                    .any(|value| value.is_empty() || value.contains('.'))
                || values.windows(2).any(|pair| pair[0] == pair[1])
            {
                return Err(error(
                    ErrorCode::BadRequest,
                    "extensions はドットを付けず重複せず1～20個指定してください",
                ));
            }
            self.extensions = Some(values);
        }
        Ok(self)
    }

    fn matches(&self, item: &FolderItem) -> bool {
        self.kinds
            .as_ref()
            .is_none_or(|kinds| kinds.contains(&item.kind))
            && self
                .name_contains
                .as_ref()
                .is_none_or(|name| item.name.to_lowercase().contains(&name.to_lowercase()))
            && self.extensions.as_ref().is_none_or(|extensions| {
                item.name
                    .rsplit_once('.')
                    .is_some_and(|(_, extension)| extensions.contains(&extension.to_lowercase()))
            })
            && self.tagged.is_none_or(|tagged| {
                item.tags
                    .as_ref()
                    .is_some_and(|tags| !tags.is_empty() == tagged)
            })
            && if self.min_stars.is_some() || self.max_stars.is_some() {
                item.rating.is_some_and(|rating| {
                    rating >= self.min_stars.unwrap_or(0) && rating <= self.max_stars.unwrap_or(5)
                })
            } else {
                true
            }
    }
}

#[derive(Default)]
pub(super) struct FolderOptions {
    pub recursive: bool,
    pub max_depth: Option<u32>,
    pub name_contains: Option<String>,
    pub extensions: Option<Vec<String>>,
    pub tagged: Option<bool>,
}

struct FolderListing {
    items: Vec<FolderItem>,
    applied: bool,
    args: serde_json::Value,
    // Index ranges identify descendants without path prefix guesses.
    folders: Vec<(FolderSummaryFolder, std::ops::Range<usize>)>,
}

const AGENT_NESTED_LIMITS: crate::zip_loader::NestedZipLimits =
    crate::zip_loader::NestedZipLimits {
        max_archive_bytes: 64 * 1024 * 1024,
        max_total_bytes: 256 * 1024 * 1024,
    };

impl Handlers {
    #[cfg(test)]
    pub(super) fn list_folder(
        &self,
        path: String,
        subresource: Option<String>,
        min_stars: Option<u8>,
        max_stars: Option<u8>,
        kinds: Option<Vec<FolderKind>>,
        limit: Option<u32>,
        cursor: Option<String>,
        ctx: &ReadContext,
    ) -> Result<Payload, AgentError> {
        self.list_folder_with_options(
            path,
            subresource,
            min_stars,
            max_stars,
            kinds,
            FolderOptions::default(),
            limit,
            cursor,
            ctx,
        )
    }

    pub(super) fn list_folder_with_options(
        &self,
        path: String,
        subresource: Option<String>,
        min_stars: Option<u8>,
        max_stars: Option<u8>,
        kinds: Option<Vec<FolderKind>>,
        options: FolderOptions,
        limit: Option<u32>,
        cursor: Option<String>,
        ctx: &ReadContext,
    ) -> Result<Payload, AgentError> {
        folder_limit(limit)?;
        let filters = FolderFilters::new(min_stars, max_stars, kinds)?.with_extra(&options)?;
        let listing =
            self.folder_listing(&path, subresource.as_deref(), &options, &filters, ctx)?;
        let page = folder_page(
            listing.items,
            listing.applied,
            limit,
            cursor.as_deref(),
            &listing.args,
            ctx.granted.1,
            &filters,
            |items| {
                if filters.tagged.is_none() {
                    enrich_tags(items, ctx)?;
                }
                Ok(())
            },
        )?;
        ctx.checkpoint()?;
        Ok(Payload::Folder(page))
    }

    pub(super) fn summarize_folder(
        &self,
        path: String,
        subresource: Option<String>,
        min_stars: Option<u8>,
        max_stars: Option<u8>,
        kinds: Option<Vec<FolderKind>>,
        options: FolderOptions,
        ctx: &ReadContext,
    ) -> Result<Payload, AgentError> {
        let filters = FolderFilters::new(min_stars, max_stars, kinds)?.with_extra(&options)?;
        let listing =
            self.folder_listing(&path, subresource.as_deref(), &options, &filters, ctx)?;
        let summary = summarize_listing(listing, options.recursive, &filters)?;
        ctx.checkpoint()?;
        Ok(Payload::FolderSummary(summary))
    }

    fn folder_listing(
        &self,
        input: &str,
        subresource: Option<&str>,
        options: &FolderOptions,
        filters: &FolderFilters,
        ctx: &ReadContext,
    ) -> Result<FolderListing, AgentError> {
        ctx.checkpoint()?;
        if options
            .max_depth
            .is_some_and(|depth| !(1..=32).contains(&depth))
            || (!options.recursive && options.max_depth.is_some())
        {
            return Err(error(
                ErrorCode::BadRequest,
                "max_depth はサブフォルダ込みのときだけ1～32で指定してください",
            ));
        }
        let max_depth = options.max_depth.unwrap_or(32);
        let path = input_path::validate(input)?;
        let prefix = folder_prefix(subresource)?;
        let common = crate::settings_db::with_db_result(|db| db.load_into_settings())
            .map_err(|_| internal())?;
        ctx.checkpoint()?;
        let io = crate::io_semaphore::GlobalIoSemaphore::process_shared(
            common.indexer_speed_profile.io_permits(),
        );
        let _io = io
            .acquire_cancellable(crate::io_semaphore::IoPriority::Normal, &ctx.cancel)
            .ok_or_else(cancelled)?;
        let metadata = std::fs::metadata(path).map_err(|_| internal())?;
        if options.recursive && (!metadata.is_dir() || subresource.is_some()) {
            return Err(error(
                ErrorCode::BadRequest,
                "サブフォルダ込みはフォルダのときだけ指定できます",
            ));
        }
        let states = favorite_states(&common)?;
        let rating_db = optional_ratings()?;
        let mut folders = Vec::new();
        let (rows, applied) = if metadata.is_dir() {
            if subresource.is_some() {
                return Err(bad_folder());
            }
            let mut walk = FolderWalk {
                common: &common,
                states: &states,
                ratings: rating_db.as_ref(),
                folder_order: self.readers.folder_order(),
                ctx,
                max_depth: if options.recursive { max_depth } else { 0 },
                visited: 0,
                rows: Vec::new(),
                folders: Vec::new(),
            };
            walk.visit(path, 0)?;
            folders = walk.folders;
            (walk.rows, false)
        } else {
            let settings = folder_settings(&common, &states, path);
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
        let mut items = enrich_ratings_with_db(rows, rating_db.as_ref(), ctx)?;
        if filters.tagged.is_some() {
            enrich_tags(&mut items, ctx)?;
        }
        let args = serde_json::json!([
            physical_identity(path),
            prefix
                .iter()
                .map(|segment| segment.to_lowercase())
                .collect::<Vec<_>>(),
            filters,
            options.recursive,
            max_depth
        ]);
        Ok(FolderListing {
            items,
            applied,
            args,
            folders,
        })
    }
}

fn summarize_listing(
    listing: FolderListing,
    recursive: bool,
    filters: &FolderFilters,
) -> Result<FolderSummary, AgentError> {
    let mut summary = FolderSummary {
        total: 0,
        kinds: Default::default(),
        stars: [0; 6],
        folders: recursive.then(Vec::new),
        folders_truncated: false,
        book_composition_applied: listing.applied,
    };
    // Prefix counts make each direct child's subtree count constant-time.
    let mut counts = Vec::with_capacity(listing.items.len() + 1);
    counts.push(0usize);
    for item in &listing.items {
        let matches = filters.matches(item);
        counts.push(counts.last().unwrap() + usize::from(matches));
        if matches {
            summary.total += 1;
            *summary.kinds.entry(item.kind).or_default() += 1;
            if let Some(rating) = item.rating {
                summary.stars[usize::from(rating)] += 1;
            }
        }
    }
    if let Some(folders) = &mut summary.folders {
        summary.folders_truncated = listing.folders.len() > 200;
        for (mut folder, range) in listing.folders.into_iter().take(200) {
            folder.total = counts[range.end] - counts[range.start];
            folders.push(folder);
        }
    }
    while mcp_content_chars(&summary).map_err(|_| internal())? > MAX_PAGE_CHARS {
        if summary.folders.as_mut().and_then(Vec::pop).is_none() {
            return Err(internal());
        }
        summary.folders_truncated = true;
    }
    Ok(summary)
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

type FavoriteStates = HashMap<uuid::Uuid, crate::settings::FavoriteViewState>;

fn favorite_states(settings: &Settings) -> Result<FavoriteStates, AgentError> {
    if !settings.remember_favorite_view_state
        || !crate::adjustment_db::AdjustmentDb::db_path()
            .try_exists()
            .map_err(|_| internal())?
    {
        return Ok(HashMap::new());
    }
    Ok(crate::adjustment_db::AdjustmentDb::open_readonly()
        .map_err(|_| internal())?
        .load_all_favorite_view_states())
}

fn folder_settings(common: &Settings, states: &FavoriteStates, path: &Path) -> Settings {
    let mut settings = common.clone();
    if let Some(id) = crate::final_composite::active_favorite_default_id_for_path(
        path,
        &settings.favorites,
        None,
        |id| states.contains_key(&id),
    ) {
        settings.apply_favorite_view_overlay(id, &states[&id]);
    }
    settings
}

fn optional_ratings() -> Result<Option<crate::rating_db::RatingDb>, AgentError> {
    let path = crate::rating_db::RatingDb::db_path();
    if !path.try_exists().map_err(|_| internal())? {
        return Ok(None);
    }
    #[cfg(test)]
    DB_OPENS.with(|counts| counts.set((counts.get().0 + 1, counts.get().1)));
    crate::rating_db::RatingDb::open_readonly(path)
        .map(Some)
        .map_err(|_| internal())
}

#[cfg(test)]
thread_local! { static DB_OPENS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) }; }

const MAX_RECURSIVE_FOLDERS: usize = 1_000;
const MAX_RECURSIVE_ROWS: usize = 50_000;

fn traversal_limit() -> AgentError {
    error(
        ErrorCode::BadRequest,
        "サブフォルダ込みの対象が多すぎます (フォルダ 1,000・項目 50,000 まで)。max_depth を小さくするか、下のフォルダを指定してください",
    )
}

fn folder_read_error(path: &Path, child: bool, cause: std::io::Error) -> AgentError {
    if child && cause.kind() == std::io::ErrorKind::PermissionDenied {
        let path = path.to_string_lossy();
        let skip = path.chars().count().saturating_sub(1000);
        let path = path.chars().skip(skip).collect::<String>();
        error(
            ErrorCode::BadRequest,
            &format!(
                "サブフォルダ {path} を読めません。そのフォルダを含まない範囲 (下のフォルダごと、または max_depth) を指定してください"
            ),
        )
    } else {
        internal()
    }
}

struct FolderWalk<'a> {
    common: &'a Settings,
    states: &'a FavoriteStates,
    ratings: Option<&'a crate::rating_db::RatingDb>,
    folder_order: Option<crate::folder_order::FolderOrderHandle>,
    ctx: &'a ReadContext,
    max_depth: u32,
    visited: usize,
    rows: Vec<FolderRow>,
    folders: Vec<(FolderSummaryFolder, std::ops::Range<usize>)>,
}

impl FolderWalk<'_> {
    fn visit(&mut self, path: &Path, depth: u32) -> Result<(), AgentError> {
        self.ctx.checkpoint()?;
        self.visited += 1;
        if self.max_depth > 0 && self.visited > MAX_RECURSIVE_FOLDERS {
            return Err(traversal_limit());
        }
        let settings = folder_settings(self.common, self.states, path);
        let rows = physical_folder(
            path,
            &settings,
            self.folder_order.as_ref(),
            self.ctx,
            self.ratings,
            depth > 0,
        )?;
        // Count the whole base listing even before descending or filtering it.
        if self.max_depth > 0 && self.rows.len() + rows.len() > MAX_RECURSIVE_ROWS {
            return Err(traversal_limit());
        }
        for row in rows {
            let child = (row.item.kind == FolderKind::Folder && depth < self.max_depth)
                .then(|| PathBuf::from(&row.item.path));
            let direct =
                (depth == 0 && row.item.kind == FolderKind::Folder).then(|| FolderSummaryFolder {
                    path: row.item.path.clone(),
                    name: row.item.name.clone(),
                    total: 0,
                });
            self.rows.push(row);
            if self.max_depth > 0 && self.rows.len() > MAX_RECURSIVE_ROWS {
                return Err(traversal_limit());
            }
            let start = self.rows.len();
            if let Some(child) = child {
                if enter_folder(&child).map_err(|cause| folder_read_error(&child, true, cause))? {
                    self.visit(&child, depth + 1)?;
                }
            }
            if let Some(folder) = direct {
                self.folders.push((folder, start..self.rows.len()));
            }
        }
        Ok(())
    }
}

fn enter_folder(path: &Path) -> std::io::Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::{
            fs::{MetadataExt, OpenOptionsExt},
            io::AsRawHandle,
        };
        use windows::Win32::{
            Foundation::HANDLE,
            Storage::FileSystem::{
                FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
                FILE_FLAG_OPEN_REPARSE_POINT, FileAttributeTagInfo, GetFileInformationByHandleEx,
            },
        };
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 {
            return Ok(true);
        }
        let file = std::fs::OpenOptions::new()
            .access_mode(0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(path)?;
        let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
        unsafe {
            GetFileInformationByHandleEx(
                HANDLE(file.as_raw_handle()),
                FileAttributeTagInfo,
                (&raw mut info).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        }
        .map_err(|cause| std::io::Error::from_raw_os_error(cause.code().0 & 0xffff))?;
        enter_reparse_tag(Ok(info.ReparseTag))
    }
    #[cfg(not(windows))]
    {
        Ok(!std::fs::symlink_metadata(path)?.file_type().is_symlink())
    }
}

fn name_surrogate(tag: u32) -> bool {
    tag & 0x2000_0000 != 0
}

fn enter_reparse_tag(tag: std::io::Result<u32>) -> std::io::Result<bool> {
    tag.map(|tag| !name_surrogate(tag))
}

fn physical_folder(
    path: &Path,
    settings: &Settings,
    folder_order: Option<&crate::folder_order::FolderOrderHandle>,
    ctx: &ReadContext,
    ratings: Option<&crate::rating_db::RatingDb>,
    child: bool,
) -> Result<Vec<FolderRow>, AgentError> {
    let scan = crate::app::folder_scan::scan_directory_with_settings_cancel(
        path,
        settings,
        Some(&ctx.cancel),
    );
    ctx.checkpoint()?;
    let scan = scan.map_err(|cause| folder_read_error(path, child, cause))?;
    // This handler runs on the read worker. The App-owned actor serializes the
    // read behind accepted saves/renames; this lane never opens folder_order.db.
    let folder_order =
        crate::folder_order::FolderOrderListingInput::read_worker(folder_order, path);
    ctx.checkpoint()?;
    if let Some(cause) = folder_order.error() {
        crate::logger::log(format!(
            "agent_ipc: folder order unavailable for {}: {cause}",
            path.display()
        ));
    }
    let known_metadata = scan
        .precise_stamps
        .iter()
        .filter_map(|(key, stamp)| stamp.is_some().then_some(*key))
        .collect::<std::collections::HashSet<_>>();
    let rows = materialize::materialize_local_folder_listing_with_order(
        path,
        scan,
        settings,
        folder_order,
        crate::rating_sort::ListingOrderRequest::from_settings(settings),
        |keys| {
            ratings
                .ok_or_else(|| "rating DB is absent".to_owned())?
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
    folder_items(rows.items, metas, &HashMap::new())
}

fn archive_folder(
    path: &Path,
    requested: &[String],
    settings: &Settings,
    snapshot: &CompositionSnapshot,
    ctx: &ReadContext,
) -> Result<(Vec<FolderRow>, bool), AgentError> {
    let enumeration =
        crate::zip_loader::enumerate_image_entries_detailed_readonly_with_cancel_and_limits(
            path,
            Some(&ctx.cancel),
            Some(AGENT_NESTED_LIMITS),
        );
    ctx.checkpoint()?;
    let mut enumeration = enumeration.map_err(|cause| {
        if crate::zip_loader::is_nested_zip_limit_error(&cause) {
            error(ErrorCode::BadRequest, "大きすぎる入れ子の ZIP があります")
        } else {
            internal()
        }
    })?;
    // prepare consumes the enumeration. Retain image aliases, including the
    // full legacy ancestry of nested ZIP entries, without migrating any key.
    let legacy_names = std::mem::take(&mut enumeration.legacy_renames)
        .into_iter()
        .map(|(legacy, current)| (current, legacy))
        .collect::<HashMap<_, _>>();
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
    Ok((
        folder_items(rows.items, rows.metas, &legacy_names)?,
        applied,
    ))
}

fn document_folder(
    path: &Path,
    metadata: &std::fs::Metadata,
    settings: &Settings,
    snapshot: &CompositionSnapshot,
    ctx: &ReadContext,
) -> Result<(Vec<FolderRow>, bool), AgentError> {
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
    Ok((
        folder_items(rows.items, rows.metas, &HashMap::new())?,
        applied,
    ))
}

fn folder_items(
    items: Vec<GridItem>,
    metas: Vec<Option<(i64, i64)>>,
    legacy_names: &HashMap<String, String>,
) -> Result<Vec<FolderRow>, AgentError> {
    if items.len() != metas.len() {
        return Err(internal());
    }
    items
        .into_iter()
        .zip(metas)
        .map(|(item, meta)| {
            let name = item.name().into_owned();
            let rating_key = item.accepts_rating().then(|| {
                let legacy = match &item {
                    GridItem::ZipImage {
                        zip_path,
                        entry_name,
                    } => legacy_names
                        .get(entry_name)
                        .map(|name| crate::adjustment_db::zip_entry_key(zip_path, name)),
                    // ZipDir aliases are deliberately unsupported, matching the grid.
                    _ => None,
                };
                RatingKey::for_item(&item, legacy)
            });
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
            let tags = subresource.is_none().then(Vec::new);
            Ok(FolderRow {
                rating_key,
                item: FolderItem {
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
                    rating: None,
                    tags,
                },
            })
        })
        .collect()
}

fn enrich_ratings_with_db(
    rows: Vec<FolderRow>,
    db: Option<&crate::rating_db::RatingDb>,
    ctx: &ReadContext,
) -> Result<Vec<FolderItem>, AgentError> {
    ctx.checkpoint()?;
    let keys = rows
        .iter()
        .filter_map(|row| row.rating_key.as_ref())
        .flat_map(RatingKey::keys)
        .cloned()
        .collect::<Vec<_>>();
    let ratings = db.map(|db| db.get_many(&keys)).unwrap_or_default();
    ctx.checkpoint()?;
    Ok(rows
        .into_iter()
        .map(|mut row| {
            row.item.rating = row.rating_key.map(|key| key.resolve(&ratings));
            row.item
        })
        .collect())
}

fn enrich_tags(items: &mut [FolderItem], ctx: &ReadContext) -> Result<(), AgentError> {
    ctx.checkpoint()?;
    let db = optional_tags(&crate::tags_db::TagsDb::db_path())?;
    #[cfg(test)]
    if db.is_some() {
        DB_OPENS.with(|counts| counts.set((counts.get().0, counts.get().1 + 1)));
    }
    let keys = items
        .iter()
        .filter(|item| item.subresource.is_none())
        .map(|item| crate::tags_db::item_key_for_path(Path::new(&item.path)))
        .collect::<Vec<_>>();
    ctx.checkpoint()?;
    let tags = db
        .map(|db| db.get_many_display_tags(&keys))
        .unwrap_or_default();
    ctx.checkpoint()?;
    for item in items.iter_mut().filter(|item| item.subresource.is_none()) {
        let key = crate::tags_db::item_key_for_path(Path::new(&item.path));
        item.tags = Some(tags.get(&key).cloned().unwrap_or_default());
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct FolderCursor {
    version: u32,
    tool: String,
    args_digest: String,
    listing_digest: String,
    returned: usize,
    #[serde(default)]
    permission_epoch: u64,
}

fn folder_cursor(
    args_digest: &str,
    listing_digest: &str,
    returned: usize,
    permission_epoch: u64,
) -> String {
    serde_json::to_vec(&FolderCursor {
        version: 5,
        tool: "list_folder".into(),
        args_digest: args_digest.into(),
        listing_digest: listing_digest.into(),
        returned,
        permission_epoch,
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
    permission_epoch: u64,
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
    if cursor.version != 5 || cursor.tool != "list_folder" || cursor.args_digest != args {
        return Err(bad());
    }
    if cursor.permission_epoch != permission_epoch {
        return Err(error(
            ErrorCode::PermissionChanged,
            "許可が変わりました。最初からやり直してください",
        ));
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
    permission_epoch: u64,
    filters: &FolderFilters,
    read_tags: impl FnOnce(&mut [FolderItem]) -> Result<(), AgentError>,
) -> Result<FolderPage, AgentError> {
    let limit = folder_limit(limit)?;
    let args_digest = wire_digest(args).map_err(|_| internal())?;
    let identity = items
        .iter()
        .map(|item| (physical_identity(Path::new(&item.path)), &item.subresource))
        .collect::<Vec<_>>();
    let listing_digest = wire_digest(&identity).map_err(|_| internal())?;
    let offset = folder_offset(
        cursor,
        &args_digest,
        &listing_digest,
        items.len(),
        permission_epoch,
    )?;
    let mut matching = items
        .into_iter()
        .enumerate()
        .skip(offset)
        .filter(|(_, item)| filters.matches(item));
    let selected = matching.by_ref().take(limit).collect::<Vec<_>>();
    let more = matching.next().is_some();
    let (indices, mut items): (Vec<_>, Vec<_>) = selected.into_iter().unzip();
    // Tags cannot affect membership, and only the prospective page needs them.
    read_tags(&mut items)?;
    let items = items
        .into_iter()
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
        page.truncated = more || page.items.len() < indices.len();
        page.cursor = (page.truncated && !page.items.is_empty()).then(|| {
            folder_cursor(
                &args_digest,
                &listing_digest,
                indices[page.items.len() - 1] + 1,
                permission_epoch,
            )
        });
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
