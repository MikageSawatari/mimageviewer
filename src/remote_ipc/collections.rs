use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mimageviewer_ipc::{
    CollectionError, CollectionErrorCode, CollectionKind, CollectionPayload, CollectionRequest,
    CollectionResponse, FavoriteSearchIndexState, FavoriteSearchKind, FavoriteSearchPayload,
    FavoriteSearchRequest, FavoriteSearchResponse, HomePayload, HomeResponse, PageGroup,
    PlaceSummary, RemoteAddress, RemoteEntry, RemoteEntryKind, RemoteReadingDirection,
    RemoteSpreadMode, RemoteTagChoice, SmartFolderSummary, TagBrowsePayload, TagBrowseRequest,
    TagBrowseResponse, TagIndexState, TagItemKind, TagItemsPayload, TagItemsRequest,
    TagItemsResponse,
};
use rayon::prelude::*;

use crate::grid_item::GridItem;
use crate::search_index_db::{IndexEntry, IndexKind, SEARCH_RESULT_LIMIT, SearchIndexDb};
use crate::settings::Settings;

const MAX_REMOTE_COLLECTION_ENTRIES: usize = 100_000;
const MAX_REMOTE_TAG_CHOICES: usize = 2000;
const PAGE_GROUP_JSON_OVERHEAD_PER_IMAGE: usize = 32;

pub(super) struct CollectionEngine {
    settings: CollectionSettingsSource,
    sort_settings: super::RemoteSortSettingsSource,
    favorites: Arc<super::live_favorites::LiveFavorites>,
}

enum CollectionSettingsSource {
    Live,
    #[cfg(test)]
    Snapshot(Settings),
}

impl CollectionSettingsSource {
    fn load(&self) -> Result<Settings, String> {
        match self {
            Self::Live => crate::settings_db::with_db_result(|db| db.load_into_settings())
                .map_err(|error| error.to_string()),
            #[cfg(test)]
            Self::Snapshot(settings) => Ok(settings.clone()),
        }
    }
}

#[derive(Clone)]
struct CandidateEntry {
    path: PathBuf,
    name: String,
    kind: RemoteEntryKind,
    detail: Option<String>,
    progress_current: Option<u64>,
    progress_total: Option<u64>,
    rating: Option<u8>,
}

#[derive(Clone, Copy, Default)]
struct CollectionSpreadRequest {
    spread_mode: Option<RemoteSpreadMode>,
    reading_direction: Option<RemoteReadingDirection>,
    force_single_page: bool,
}

struct CollectionSpreadPayload {
    configured: RemoteSpreadMode,
    effective: RemoteSpreadMode,
    reading_direction: RemoteReadingDirection,
    image_count: usize,
    groups: Vec<PageGroup>,
}

impl CollectionEngine {
    #[cfg(test)]
    pub(super) fn new(settings: Settings) -> Self {
        let favorites = super::live_favorites::LiveFavorites::snapshot(settings.favorites.clone());
        Self::new_with_favorites(settings, favorites)
    }

    #[cfg(test)]
    pub(super) fn new_with_favorites(
        settings: Settings,
        favorites: Arc<super::live_favorites::LiveFavorites>,
    ) -> Self {
        Self {
            sort_settings: super::RemoteSortSettingsSource::Snapshot(settings.sort_order),
            settings: CollectionSettingsSource::Snapshot(settings),
            favorites,
        }
    }

    pub(super) fn new_with_live_favorites(
        _settings: Settings,
        favorites: Arc<super::live_favorites::LiveFavorites>,
    ) -> Self {
        Self {
            settings: CollectionSettingsSource::Live,
            sort_settings: super::RemoteSortSettingsSource::Live,
            favorites,
        }
    }

    pub(super) fn home(&self) -> HomeResponse {
        let settings = match self.load_settings() {
            Ok(settings) => settings,
            Err(error) => return HomeResponse::Error(error),
        };
        HomeResponse::Success(HomePayload {
            smart_folders: settings
                .smart_folders
                .iter()
                .map(|definition| SmartFolderSummary {
                    id: definition.id.to_string(),
                    name: definition.name.clone(),
                })
                .collect(),
            places: visible_places(&settings),
        })
    }

    pub(super) fn collection(&self, request: CollectionRequest) -> CollectionResponse {
        let settings = match self.load_settings() {
            Ok(settings) => settings,
            Err(error) => return CollectionResponse::Error(error),
        };
        let sort_order = match self.sort_settings.load() {
            Ok(sort_order) => sort_order,
            Err(error) => {
                return CollectionResponse::Error(internal_error(
                    "最新の並び順を読み込めませんでした",
                    error,
                ));
            }
        };
        let spread_request = CollectionSpreadRequest {
            spread_mode: request.spread_mode,
            reading_direction: request.reading_direction,
            force_single_page: request.force_single_page,
        };
        let result = match request.kind {
            CollectionKind::DriveList => self.drive_list(&settings, sort_order, spread_request),
            CollectionKind::ReadingHistory => {
                self.reading_history(&settings, sort_order, spread_request)
            }
            CollectionKind::Rating { stars } => {
                self.rating(&settings, stars, sort_order, spread_request)
            }
            CollectionKind::Bookshelf => self.bookshelf(&settings, sort_order, spread_request),
            CollectionKind::Bookmarks => self.bookmarks(&settings, sort_order, spread_request),
            CollectionKind::SmartFolder { definition_id } => {
                self.smart_folder(&settings, &definition_id, sort_order, spread_request)
            }
        };
        match result {
            Ok(payload) => CollectionResponse::Success(payload),
            Err(error) => CollectionResponse::Error(error),
        }
    }

    fn load_settings(&self) -> Result<Settings, CollectionError> {
        self.settings
            .load()
            .map_err(|error| internal_error("最新の一覧設定を読み込めませんでした", error))
    }

    /// Favorite search stays in CollectionEngine because it shares the response bound,
    /// aspect ratio, and fixed-sort contract with the other collections.
    pub(super) fn favorite_search(&self, request: FavoriteSearchRequest) -> FavoriteSearchResponse {
        match self.favorite_search_payload(request) {
            Ok(payload) => FavoriteSearchResponse::Success(payload),
            Err(error) => FavoriteSearchResponse::Error(error),
        }
    }

    fn favorite_search_payload(
        &self,
        request: FavoriteSearchRequest,
    ) -> Result<FavoriteSearchPayload, CollectionError> {
        if request.query.trim().is_empty() {
            return Err(CollectionError::new(
                CollectionErrorCode::BadRequest,
                "検索語句を入力してください",
            ));
        }
        if request.query.chars().count() > 200 {
            return Err(CollectionError::new(
                CollectionErrorCode::BadRequest,
                "検索語句は 200 文字以内で入力してください",
            ));
        }
        let settings = self.load_settings()?;
        let sort_order = self
            .sort_settings
            .load()
            .map_err(|error| internal_error("最新の並び順を読み込めませんでした", error))?;
        let favorites = self.favorites.current().map_err(|error| {
            crate::logger::log(format!("remote_ipc: {error}"));
            CollectionError::new(
                CollectionErrorCode::Internal,
                "最新の閲覧起点を読み込めませんでした",
            )
        })?;
        if !favorites
            .iter()
            .any(|favorite| favorite.auto_index_structure)
        {
            return Ok(self.favorite_search_state_payload(
                &settings,
                sort_order,
                FavoriteSearchIndexState::Disabled,
            ));
        }

        let db = match SearchIndexDb::open_readonly() {
            Ok(db) => db,
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: favorite search index open failed: {error}"
                ));
                return Ok(self.favorite_search_state_payload(
                    &settings,
                    sort_order,
                    FavoriteSearchIndexState::Unavailable,
                ));
            }
        };
        let favorite_paths = favorites
            .iter()
            .map(|favorite| favorite.path.clone())
            .collect::<Vec<_>>();
        let kind = match request.kind {
            FavoriteSearchKind::All => None,
            FavoriteSearchKind::Folder => Some(IndexKind::Folder),
            FavoriteSearchKind::Zip => Some(IndexKind::ZipFile),
            FavoriteSearchKind::Pdf => Some(IndexKind::PdfFile),
        };
        let index_entries = match db.search(
            &request.query,
            &favorite_paths,
            kind,
            crate::search_query::MatchMode::And,
        ) {
            Ok(entries) => entries,
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: favorite search index read failed: {error}"
                ));
                return Ok(self.favorite_search_state_payload(
                    &settings,
                    sort_order,
                    FavoriteSearchIndexState::Unavailable,
                ));
            }
        };
        let (entries, truncated) = map_favorite_search_entries(index_entries);
        Ok(FavoriteSearchPayload {
            listing: self.favorite_search_listing(&settings, sort_order, entries, truncated),
            index_state: FavoriteSearchIndexState::Ready,
        })
    }

    fn favorite_search_state_payload(
        &self,
        settings: &Settings,
        sort_order: crate::settings::SortOrder,
        index_state: FavoriteSearchIndexState,
    ) -> FavoriteSearchPayload {
        FavoriteSearchPayload {
            listing: self.favorite_search_listing(settings, sort_order, Vec::new(), false),
            index_state,
        }
    }

    fn favorite_search_listing(
        &self,
        settings: &Settings,
        sort_order: crate::settings::SortOrder,
        entries: Vec<RemoteEntry>,
        truncated: bool,
    ) -> CollectionPayload {
        let (entries, truncated) = finalize_remote_entries(settings, entries, truncated);
        let entry_limit =
            super::response_entry_limit(MAX_REMOTE_COLLECTION_ENTRIES, entries.len(), truncated);
        collection_payload(
            settings,
            "検索結果",
            super::remote_grid_sort_state(sort_order, Some(super::FIXED_LIST_SORT_LOCK_REASON)),
            entries,
            entry_limit,
            truncated,
            CollectionSpreadRequest::default(),
        )
    }

    pub(super) fn tag_browse(&self, _request: TagBrowseRequest) -> TagBrowseResponse {
        match self.load_settings() {
            Ok(settings) => TagBrowseResponse::Success(self.tag_browse_payload(&settings)),
            Err(error) => TagBrowseResponse::Error(error),
        }
    }

    fn tag_browse_payload(&self, settings: &Settings) -> TagBrowsePayload {
        let db_path = crate::tags_db::TagsDb::db_path();
        if !db_path.try_exists().unwrap_or(false) {
            return empty_tag_browse_payload(TagIndexState::Unavailable);
        }
        let db = match crate::tags_db::TagsDb::open_readonly(&db_path) {
            Ok(db) => db,
            Err(error) => {
                crate::logger::log(format!("remote_ipc: tags index open failed: {error}"));
                return empty_tag_browse_payload(TagIndexState::Unavailable);
            }
        };
        tag_browse_payload_from_summaries(db.tag_summaries(), &settings.tags)
    }

    pub(super) fn tag_items(&self, request: TagItemsRequest) -> TagItemsResponse {
        match self.tag_items_payload(request) {
            Ok(payload) => TagItemsResponse::Success(payload),
            Err(error) => TagItemsResponse::Error(error),
        }
    }

    fn tag_items_payload(
        &self,
        request: TagItemsRequest,
    ) -> Result<TagItemsPayload, CollectionError> {
        validate_tag_items_request(&request)?;
        let settings = self.load_settings()?;
        let sort_order = self
            .sort_settings
            .load()
            .map_err(|error| internal_error("最新の並び順を読み込めませんでした", error))?;
        let db_path = crate::tags_db::TagsDb::db_path();
        if !db_path.try_exists().unwrap_or(false) {
            return Ok(self.tag_items_state_payload(
                &settings,
                sort_order,
                TagIndexState::Unavailable,
            ));
        }
        let db = match crate::tags_db::TagsDb::open_readonly(&db_path) {
            Ok(db) => db,
            Err(error) => {
                crate::logger::log(format!("remote_ipc: tags index open failed: {error}"));
                return Ok(self.tag_items_state_payload(
                    &settings,
                    sort_order,
                    TagIndexState::Unavailable,
                ));
            }
        };
        match db.has_any_tags() {
            Ok(true) => {}
            Ok(false) => {
                return Ok(self.tag_items_state_payload(
                    &settings,
                    sort_order,
                    TagIndexState::Empty,
                ));
            }
            Err(error) => {
                crate::logger::log(format!("remote_ipc: tags index query failed: {error}"));
                return Ok(self.tag_items_state_payload(
                    &settings,
                    sort_order,
                    TagIndexState::Unavailable,
                ));
            }
        }
        self.tag_items_from_db(&settings, &db, &request, sort_order)
    }

    fn tag_items_from_db(
        &self,
        settings: &Settings,
        db: &crate::tags_db::TagsDb,
        request: &TagItemsRequest,
        sort_order: crate::settings::SortOrder,
    ) -> Result<TagItemsPayload, CollectionError> {
        let key_scan_limit = tag_item_key_scan_limit(request.kind);
        let item_keys =
            crate::tag_view::select_tag_view_item_keys(db, request.tag.trim(), key_scan_limit + 1);
        let (entries, truncated) = map_tag_item_keys(
            item_keys,
            request.kind,
            key_scan_limit,
            !settings.archive_file_handling_ignores_convertible(),
        );
        Ok(TagItemsPayload {
            listing: self.tag_items_listing(settings, sort_order, entries, truncated),
            state: TagIndexState::Ready,
        })
    }

    fn tag_items_state_payload(
        &self,
        settings: &Settings,
        sort_order: crate::settings::SortOrder,
        state: TagIndexState,
    ) -> TagItemsPayload {
        TagItemsPayload {
            listing: self.tag_items_listing(settings, sort_order, Vec::new(), false),
            state,
        }
    }

    fn tag_items_listing(
        &self,
        settings: &Settings,
        sort_order: crate::settings::SortOrder,
        entries: Vec<RemoteEntry>,
        truncated: bool,
    ) -> CollectionPayload {
        let (entries, truncated) = finalize_remote_entries(settings, entries, truncated);
        let entry_limit =
            super::response_entry_limit(MAX_REMOTE_COLLECTION_ENTRIES, entries.len(), truncated);
        collection_payload(
            settings,
            "タグの項目",
            super::remote_grid_sort_state(sort_order, Some(super::FIXED_LIST_SORT_LOCK_REASON)),
            entries,
            entry_limit,
            truncated,
            CollectionSpreadRequest::default(),
        )
    }

    fn reading_history(
        &self,
        settings: &Settings,
        sort_order: crate::settings::SortOrder,
        spread_request: CollectionSpreadRequest,
    ) -> Result<CollectionPayload, CollectionError> {
        let rows = if crate::reading_history_db::ReadingHistoryDb::db_path()
            .try_exists()
            .unwrap_or(false)
        {
            crate::reading_history_db::ReadingHistoryDb::open_readonly()
                .and_then(|db| db.list_recent(settings.reading_history_limit))
                .map_err(|error| internal_error("閲覧履歴を読み込めませんでした", error))?
        } else {
            Vec::new()
        };
        let candidates = rows
            .into_iter()
            .map(candidate_from_reading_history_entry)
            .collect();
        self.payload(
            settings,
            "閲覧履歴",
            candidates,
            sort_order,
            Some(super::FIXED_LIST_SORT_LOCK_REASON),
            spread_request,
        )
    }

    fn drive_list(
        &self,
        settings: &Settings,
        sort_order: crate::settings::SortOrder,
        spread_request: CollectionSpreadRequest,
    ) -> Result<CollectionPayload, CollectionError> {
        self.drive_list_from_paths(
            settings,
            sort_order,
            crate::known_folders::available_drives(),
            spread_request,
        )
    }

    fn drive_list_from_paths(
        &self,
        settings: &Settings,
        sort_order: crate::settings::SortOrder,
        drives: Vec<PathBuf>,
        spread_request: CollectionSpreadRequest,
    ) -> Result<CollectionPayload, CollectionError> {
        let candidates = drives
            .into_iter()
            .map(|path| CandidateEntry {
                name: path.to_string_lossy().into_owned(),
                path,
                kind: RemoteEntryKind::Folder,
                detail: None,
                progress_current: None,
                progress_total: None,
                rating: None,
            })
            .collect();
        self.payload(
            settings,
            "ドライブ一覧",
            candidates,
            sort_order,
            Some(super::FIXED_LIST_SORT_LOCK_REASON),
            spread_request,
        )
    }

    fn rating(
        &self,
        settings: &Settings,
        stars: u8,
        sort_order: crate::settings::SortOrder,
        spread_request: CollectionSpreadRequest,
    ) -> Result<CollectionPayload, CollectionError> {
        if !(1..=5).contains(&stars) {
            return Err(CollectionError::new(
                CollectionErrorCode::BadRequest,
                "レーティングは 1〜5 を指定してください",
            ));
        }
        let db_path = crate::rating_db::RatingDb::db_path();
        let mut rows = if db_path.try_exists().unwrap_or(false) {
            let db = crate::rating_db::RatingDb::open_readonly(&db_path)
                .map_err(|error| internal_error("レーティングを読み込めませんでした", error))?;
            db.list_by_stars(stars)
                .map_err(|error| internal_error("レーティングを読み込めませんでした", error))?
                .iter()
                .filter_map(crate::rating_view::rating_row_to_view_row)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        crate::rating_view::sort_rows(&mut rows, crate::rating_view::RatingViewSort::RatedAtDesc);
        let candidates = rows
            .into_iter()
            .filter_map(|row| candidate_from_grid_item(row.item, None, Some(stars)))
            .collect();
        self.payload(
            settings,
            &format!("レーティング ★{stars}"),
            candidates,
            sort_order,
            Some(super::FIXED_LIST_SORT_LOCK_REASON),
            spread_request,
        )
    }

    fn bookmarks(
        &self,
        settings: &Settings,
        sort_order: crate::settings::SortOrder,
        spread_request: CollectionSpreadRequest,
    ) -> Result<CollectionPayload, CollectionError> {
        let rows = crate::bookmark_browser::build_rows_readonly()
            .map_err(|error| internal_error("ブックマークを読み込めませんでした", error))?;
        let candidates = rows
            .into_iter()
            .map(|row| {
                let name = row.details_name();
                let detail = Some(row.position_label());
                match row.source {
                    crate::bookmark_browser::BookmarkRowSource::Media {
                        path, is_audio, ..
                    } => CandidateEntry {
                        name,
                        path,
                        kind: if is_audio {
                            RemoteEntryKind::Audio
                        } else {
                            RemoteEntryKind::Video
                        },
                        detail,
                        progress_current: None,
                        progress_total: None,
                        rating: None,
                    },
                    crate::bookmark_browser::BookmarkRowSource::Book(bookmark) => CandidateEntry {
                        name,
                        path: bookmark.container_path,
                        kind: match bookmark.container_kind {
                            crate::book_bookmarks::BookContainerKind::CompiledBook
                            | crate::book_bookmarks::BookContainerKind::ImageFolder => {
                                RemoteEntryKind::Folder
                            }
                            crate::book_bookmarks::BookContainerKind::Zip => RemoteEntryKind::Zip,
                            crate::book_bookmarks::BookContainerKind::Pdf => RemoteEntryKind::Pdf,
                            crate::book_bookmarks::BookContainerKind::OtherArchive => {
                                RemoteEntryKind::Archive
                            }
                        },
                        detail,
                        progress_current: Some(bookmark.page_index_hint.saturating_add(1) as u64),
                        progress_total: None,
                        rating: None,
                    },
                }
            })
            .collect();
        self.payload(
            settings,
            "ブックマーク",
            candidates,
            sort_order,
            Some(super::FIXED_LIST_SORT_LOCK_REASON),
            spread_request,
        )
    }

    fn bookshelf(
        &self,
        settings: &Settings,
        sort_order: crate::settings::SortOrder,
        spread_request: CollectionSpreadRequest,
    ) -> Result<CollectionPayload, CollectionError> {
        let root = settings.books_root_path();
        let candidates = crate::books::list_books(&root)
            .map_err(|error| internal_error("本棚を読み込めませんでした", error))?
            .into_iter()
            .map(|book| CandidateEntry {
                name: book.name,
                path: book.path,
                kind: RemoteEntryKind::Folder,
                detail: Some(format!("{} ページ", book.page_count)),
                progress_current: None,
                progress_total: Some(book.page_count as u64),
                rating: None,
            })
            .collect();
        self.payload(
            settings,
            "本棚",
            candidates,
            sort_order,
            Some(super::FIXED_LIST_SORT_LOCK_REASON),
            spread_request,
        )
    }

    fn smart_folder(
        &self,
        settings: &Settings,
        definition_id: &str,
        sort_order: crate::settings::SortOrder,
        spread_request: CollectionSpreadRequest,
    ) -> Result<CollectionPayload, CollectionError> {
        let id = uuid::Uuid::parse_str(definition_id).map_err(|_| {
            CollectionError::new(CollectionErrorCode::BadRequest, "ID が正しくありません")
        })?;
        let definition = settings
            .smart_folders
            .iter()
            .find(|definition| definition.id == id)
            .cloned()
            .ok_or_else(|| {
                CollectionError::new(
                    CollectionErrorCode::NotFound,
                    "スマートフォルダが見つかりません",
                )
            })?;
        let title = definition.name.clone();
        let mut evaluation_settings = settings.clone();
        evaluation_settings.sort_order = sort_order;
        let entries = crate::app::smart_folder::build_remote_smart_folder_entries(
            &evaluation_settings,
            definition,
        )
        .map_err(|error| internal_error("スマートフォルダを読み込めませんでした", error))?;
        let candidates = entries
            .into_iter()
            .map(|entry| CandidateEntry {
                name: file_name(&entry.path),
                path: entry.path,
                kind: match entry.kind {
                    crate::app::smart_folder::SmartFolderEntryKind::Folder => {
                        RemoteEntryKind::Folder
                    }
                    crate::app::smart_folder::SmartFolderEntryKind::Image => RemoteEntryKind::Image,
                    crate::app::smart_folder::SmartFolderEntryKind::Video => RemoteEntryKind::Video,
                    crate::app::smart_folder::SmartFolderEntryKind::Audio => RemoteEntryKind::Audio,
                    crate::app::smart_folder::SmartFolderEntryKind::Zip => RemoteEntryKind::Zip,
                    crate::app::smart_folder::SmartFolderEntryKind::Pdf => RemoteEntryKind::Pdf,
                    crate::app::smart_folder::SmartFolderEntryKind::Archive => {
                        RemoteEntryKind::Archive
                    }
                },
                detail: None,
                progress_current: None,
                progress_total: None,
                rating: entry.rating,
            })
            .collect();
        self.payload(
            settings,
            &title,
            candidates,
            sort_order,
            None,
            spread_request,
        )
    }

    fn payload(
        &self,
        settings: &Settings,
        title: &str,
        candidates: Vec<CandidateEntry>,
        sort_order: crate::settings::SortOrder,
        locked_reason: Option<&str>,
        spread_request: CollectionSpreadRequest,
    ) -> Result<CollectionPayload, CollectionError> {
        let entries = to_remote_entries_bounded(
            candidates
                .into_iter()
                .filter(|candidate| include_collection_candidate(settings, candidate))
                .collect(),
            MAX_REMOTE_COLLECTION_ENTRIES.saturating_add(1),
        );
        let (entries, truncated) = finalize_remote_entries(settings, entries, false);
        let entry_limit =
            super::response_entry_limit(MAX_REMOTE_COLLECTION_ENTRIES, entries.len(), truncated);
        Ok(collection_payload(
            settings,
            title,
            super::remote_grid_sort_state(sort_order, locked_reason),
            entries,
            entry_limit,
            truncated,
            spread_request,
        ))
    }
}

fn collection_payload(
    settings: &Settings,
    title: &str,
    sort_state: mimageviewer_ipc::RemoteGridSortState,
    entries: Vec<RemoteEntry>,
    entry_limit: usize,
    truncated: bool,
    spread_request: CollectionSpreadRequest,
) -> CollectionPayload {
    let spread = collection_spread_payload(&entries, spread_request);
    CollectionPayload {
        title: title.to_owned(),
        thumb_aspect_height_ratio: aggregate_thumb_aspect_height_ratio(settings),
        sort_state,
        entries,
        configured_spread_mode: spread.configured,
        effective_spread_mode: spread.effective,
        reading_direction: spread.reading_direction,
        image_count: spread.image_count,
        spread_page_gap_px: settings.spread_page_gap_px,
        page_groups: spread.groups,
        entry_limit,
        truncated,
    }
}

fn collection_spread_payload(
    entries: &[RemoteEntry],
    request: CollectionSpreadRequest,
) -> CollectionSpreadPayload {
    // Collection には spread.db の container key に相当する安定した鍵がない。
    // 永続化はせず、session request と本体の non-book 既定だけから毎回解決する。
    let defaults = crate::app::SpreadRestoreDefaults::NON_BOOK;
    let (configured, effective, reading_direction) = super::container::resolve_spread_state(
        request.spread_mode,
        request.reading_direction,
        None,
        None,
        defaults.spread_mode(),
        defaults.reading_direction(),
        request.force_single_page,
    );
    let items = entries
        .iter()
        .map(|entry| remote_entry_grid_item(entry.kind, PathBuf::from(&entry.path)))
        .collect::<Vec<_>>();
    // Single は横長判定を使わない。見開きと分割はどちらも実寸法が要る。
    let landscape = if effective == RemoteSpreadMode::Single {
        vec![false; entries.len()]
    } else {
        cached_collection_landscape_flags(entries)
    };
    let index_groups = crate::ui_fullscreen::build_remote_spread_page_groups(
        &items,
        super::container::core_spread_mode(effective),
        &landscape,
    );
    let groups = index_groups
        .into_iter()
        .filter_map(|group| {
            let pages = group
                .indices
                .into_iter()
                .filter_map(|index| entries.get(index))
                .map(|entry| RemoteAddress::file(entry.path.clone()))
                .collect::<Vec<_>>();
            let anchor = if effective.is_rtl() && pages.len() == 2 {
                pages.get(1).cloned()
            } else {
                pages.first().cloned()
            }?;
            Some(PageGroup {
                anchor,
                pages,
                slice: crate::ui_fullscreen::remote_page_slice(group.slice),
            })
        })
        .collect();
    CollectionSpreadPayload {
        configured,
        effective,
        reading_direction,
        image_count: entries
            .iter()
            .filter(|entry| entry.kind == RemoteEntryKind::Image)
            .count(),
        groups,
    }
}

fn remote_entry_grid_item(kind: RemoteEntryKind, path: PathBuf) -> GridItem {
    match kind {
        RemoteEntryKind::Folder | RemoteEntryKind::Other | RemoteEntryKind::Archive => {
            GridItem::Folder(path)
        }
        RemoteEntryKind::Image => GridItem::Image(path),
        RemoteEntryKind::Video => GridItem::Video(path),
        RemoteEntryKind::Audio => GridItem::Audio(path),
        RemoteEntryKind::Zip => GridItem::ZipFile(path),
        RemoteEntryKind::Pdf => GridItem::PdfFile(path),
    }
}

fn cached_collection_landscape_flags(entries: &[RemoteEntry]) -> Vec<bool> {
    struct ParentImages {
        path: PathBuf,
        images: Vec<(usize, String)>,
    }

    let mut parents = Vec::<ParentImages>::new();
    let mut parent_indexes = HashMap::<String, usize>::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.kind != RemoteEntryKind::Image {
            continue;
        }
        let path = Path::new(&entry.path);
        let Some(parent) = path.parent() else {
            continue;
        };
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let parent_key = crate::path_key::normalize_keep_drive(parent);
        let parent_index = *parent_indexes.entry(parent_key).or_insert_with(|| {
            let parent_index = parents.len();
            parents.push(ParentImages {
                path: parent.to_path_buf(),
                images: Vec::new(),
            });
            parent_index
        });
        parents[parent_index]
            .images
            .push((index, filename.to_owned()));
    }

    let cache_dir = crate::catalog::default_cache_dir();
    let mut landscape = vec![false; entries.len()];
    let rotation_keys = entries
        .iter()
        .map(|entry| {
            (entry.kind == RemoteEntryKind::Image)
                .then(|| GridItem::Image(PathBuf::from(&entry.path)))
                .as_ref()
                .and_then(crate::edit_source::page_key_for_grid_item)
        })
        .collect::<Vec<_>>();
    let rotations = crate::rotation_db::RotationDb::open_readonly()
        .ok()
        .map(|db| db.get_many(rotation_keys.iter().filter_map(|key| key.as_deref())))
        .unwrap_or_default();
    // 親フォルダごとに一度だけ catalog を開き、寸法列だけを一括取得する。
    // フォルダを処理し終えたら DB と寸法 map を drop するので、全親の blob/map を保持しない。
    for parent in parents {
        let catalog = crate::catalog::CatalogDb::open_existing_read_only(&cache_dir, &parent.path)
            .ok()
            .flatten();
        let cached = catalog
            .as_ref()
            .and_then(|catalog| catalog.load_source_dims().ok())
            .unwrap_or_default();
        for (index, filename) in parent.images {
            let dims = match cached.get(&filename) {
                Some(recorded) => recorded.or_else(|| {
                    catalog
                        .as_ref()
                        .and_then(|catalog| catalog.load_one(&filename).ok().flatten())
                        .and_then(|entry| crate::catalog::decode_thumb_dims(&entry.jpeg_data))
                }),
                // カタログ行が 1 つも無い場合。コンテナ側と同じ読み取りを通す。
                // ここを抜かすと、同じ本がフォルダから開けば分割され、レーティング一覧から
                // 開けば分割されない、という食い違いになる。
                None => super::container::page_dims_without_catalog(&GridItem::Image(
                    PathBuf::from(&entries[index].path),
                )),
            };
            landscape[index] = dims.is_some_and(|(width, height)| {
                let rotation = rotation_keys[index]
                    .as_ref()
                    .and_then(|key| rotations.get(key))
                    .copied()
                    .unwrap_or(crate::rotation_db::Rotation::None);
                crate::rotation_db::landscape_after_rotation(width, height, rotation)
            });
        }
    }
    landscape
}

fn empty_tag_browse_payload(state: TagIndexState) -> TagBrowsePayload {
    TagBrowsePayload {
        pinned: Vec::new(),
        recent: Vec::new(),
        popular: Vec::new(),
        all: Vec::new(),
        all_truncated: false,
        state,
    }
}

fn tag_browse_payload_from_summaries(
    summaries: Vec<crate::tags_db::TagSummary>,
    tag_defs: &[crate::settings::TagDef],
) -> TagBrowsePayload {
    if summaries.is_empty() {
        return empty_tag_browse_payload(TagIndexState::Empty);
    }
    let (pinned, recent, popular) = crate::tag_view::tag_view_menu_sections(&summaries, tag_defs);
    let all_truncated = summaries.len() > MAX_REMOTE_TAG_CHOICES;
    TagBrowsePayload {
        pinned: pinned.into_iter().map(remote_tag_choice).collect(),
        recent: recent.into_iter().map(remote_tag_choice).collect(),
        popular: popular.into_iter().map(remote_tag_choice).collect(),
        all: summaries
            .into_iter()
            .take(MAX_REMOTE_TAG_CHOICES)
            .map(|summary| RemoteTagChoice {
                name: summary.tag,
                count: summary.count,
            })
            .collect(),
        all_truncated,
        state: TagIndexState::Ready,
    }
}

fn remote_tag_choice(choice: crate::tag_view::TagViewMenuChoice) -> RemoteTagChoice {
    RemoteTagChoice {
        name: choice.name,
        count: choice.count,
    }
}

fn validate_tag_items_request(request: &TagItemsRequest) -> Result<(), CollectionError> {
    if request.tag.trim().is_empty() {
        return Err(CollectionError::new(
            CollectionErrorCode::BadRequest,
            "タグを入力してください",
        ));
    }
    if request.tag.chars().count() > 200 {
        return Err(CollectionError::new(
            CollectionErrorCode::BadRequest,
            "タグは 200 文字以内で入力してください",
        ));
    }
    Ok(())
}

fn candidate_from_tag_item_key(key: String, filter: TagItemKind) -> Option<CandidateEntry> {
    let classified = match crate::tag_view::classify_tag_view_path(PathBuf::from(key)) {
        crate::tag_view::ClassifiedTagViewPath::Existing(classified) => classified,
        // missing は外付け切断 / NAS offline でも起きる。結果から隠すだけで、
        // tags.db は変更しない。
        crate::tag_view::ClassifiedTagViewPath::Missing => return None,
    };
    let remote_kind = if classified.is_directory {
        RemoteEntryKind::Folder
    } else {
        match classified.entry.kind {
            // 本体タグビューは未知拡張子を Folder へ倒す。リモートでは実フォルダと
            // 区別し、開けないフォルダセルを作らない。
            crate::tag_view::TagViewItemKind::Folder => RemoteEntryKind::Other,
            crate::tag_view::TagViewItemKind::Image => RemoteEntryKind::Image,
            crate::tag_view::TagViewItemKind::Video => RemoteEntryKind::Video,
            crate::tag_view::TagViewItemKind::Audio => RemoteEntryKind::Audio,
            crate::tag_view::TagViewItemKind::ZipFile => RemoteEntryKind::Zip,
            crate::tag_view::TagViewItemKind::PdfFile => RemoteEntryKind::Pdf,
            crate::tag_view::TagViewItemKind::Archive(_) => RemoteEntryKind::Archive,
        }
    };
    if !tag_item_kind_matches(filter, remote_kind) {
        return None;
    }
    let path = classified.entry.path;
    Some(CandidateEntry {
        name: file_name(&path),
        path,
        kind: remote_kind,
        detail: None,
        progress_current: None,
        progress_total: None,
        rating: None,
    })
}

fn include_collection_candidate(settings: &Settings, candidate: &CandidateEntry) -> bool {
    candidate.kind != RemoteEntryKind::Archive
        || !settings.archive_file_handling_ignores_convertible()
}

fn map_tag_item_keys(
    item_keys: Vec<String>,
    filter: TagItemKind,
    key_scan_limit: usize,
    include_archives: bool,
) -> (Vec<RemoteEntry>, bool) {
    map_tag_item_keys_with_entry_limit(
        item_keys,
        filter,
        key_scan_limit,
        include_archives,
        MAX_REMOTE_COLLECTION_ENTRIES,
    )
}

fn map_tag_item_keys_with_entry_limit(
    item_keys: Vec<String>,
    filter: TagItemKind,
    key_scan_limit: usize,
    include_archives: bool,
    entry_limit: usize,
) -> (Vec<RemoteEntry>, bool) {
    let key_limit_reached = item_keys.len() > key_scan_limit;
    let mapped_limit = entry_limit.saturating_add(1);
    let mut candidates = Vec::with_capacity(mapped_limit);
    for key in item_keys.into_iter().take(key_scan_limit) {
        let Some(candidate) = candidate_from_tag_item_key(key, filter) else {
            continue;
        };
        if !include_archives && candidate.kind == RemoteEntryKind::Archive {
            continue;
        }
        candidates.push(candidate);
        if candidates.len() >= mapped_limit {
            break;
        }
    }
    let entries = to_remote_entries_bounded(candidates, mapped_limit);
    let (entries, matched_truncated) = bound_remote_entries_with_limits(
        entries,
        entry_limit,
        super::REMOTE_LIST_RESPONSE_BUDGET_BYTES,
    );
    (entries, matched_truncated || key_limit_reached)
}

fn tag_item_key_scan_limit(filter: TagItemKind) -> usize {
    if filter == TagItemKind::All {
        crate::tag_view::TAG_VIEW_RESULT_LIMIT
    } else {
        crate::tag_view::TAG_VIEW_FILTERED_KEY_SCAN_LIMIT
    }
}

fn tag_item_kind_matches(filter: TagItemKind, kind: RemoteEntryKind) -> bool {
    match filter {
        TagItemKind::All => true,
        TagItemKind::Folder => kind == RemoteEntryKind::Folder,
        TagItemKind::Image => kind == RemoteEntryKind::Image,
        TagItemKind::Video => kind == RemoteEntryKind::Video,
        TagItemKind::Audio => kind == RemoteEntryKind::Audio,
        TagItemKind::Zip => kind == RemoteEntryKind::Zip,
        TagItemKind::Pdf => kind == RemoteEntryKind::Pdf,
        TagItemKind::Archive => kind == RemoteEntryKind::Archive,
    }
}

fn bound_remote_entries(entries: Vec<RemoteEntry>) -> (Vec<RemoteEntry>, bool) {
    bound_remote_entries_with_limits(
        entries,
        MAX_REMOTE_COLLECTION_ENTRIES,
        super::REMOTE_LIST_RESPONSE_BUDGET_BYTES,
    )
}

fn bound_remote_entries_with_limits(
    entries: Vec<RemoteEntry>,
    entry_limit: usize,
    byte_budget: usize,
) -> (Vec<RemoteEntry>, bool) {
    // Brackets for entries and page_groups. Each image can become a single-page group,
    // so reserve both its pages address and anchor address. This is the worst case and
    // keeps the completed CollectionPayload below the IPC frame budget even for long paths.
    let mut estimated_bytes = 4usize;
    let mut bounded = Vec::with_capacity(entries.len().min(entry_limit));
    let mut truncated = entries.len() > entry_limit;
    for entry in entries.into_iter().take(entry_limit) {
        let group_bytes = if entry.kind == RemoteEntryKind::Image {
            let address_bytes =
                super::serialized_json_len(&RemoteAddress::file(&entry.path)).saturating_add(1);
            address_bytes
                .saturating_mul(2)
                .saturating_add(PAGE_GROUP_JSON_OVERHEAD_PER_IMAGE)
        } else {
            0
        };
        let entry_bytes = super::serialized_json_len(&entry)
            .saturating_add(1)
            .saturating_add(group_bytes);
        if estimated_bytes.saturating_add(entry_bytes) > byte_budget {
            truncated = true;
            break;
        }
        estimated_bytes = estimated_bytes.saturating_add(entry_bytes);
        bounded.push(entry);
    }
    (bounded, truncated)
}

fn finalize_remote_entries(
    settings: &Settings,
    entries: Vec<RemoteEntry>,
    source_truncated: bool,
) -> (Vec<RemoteEntry>, bool) {
    let (mut entries, initially_truncated) = bound_remote_entries(entries);
    super::RemoteThumbnailSources::for_remote_entries(settings, &entries)
        .populate_remote_entries(&mut entries);
    // A video sidecar adds another address after the first estimate, so run the exact
    // serialized-size accumulator again before publishing the payload.
    let (entries, thumbnail_truncated) = bound_remote_entries(entries);
    (
        entries,
        source_truncated || initially_truncated || thumbnail_truncated,
    )
}

fn visible_places(settings: &Settings) -> Vec<PlaceSummary> {
    crate::known_folders::location_menu_entries(settings)
        .into_iter()
        .map(|entry| match entry {
            crate::known_folders::LocationMenuEntry::DriveList => PlaceSummary::DriveList {
                name: "ドライブ一覧".to_owned(),
            },
            crate::known_folders::LocationMenuEntry::ReadingHistory => {
                PlaceSummary::ReadingHistory {
                    name: "閲覧履歴".to_owned(),
                }
            }
            crate::known_folders::LocationMenuEntry::Bookmarks => PlaceSummary::Bookmarks {
                name: "ブックマーク".to_owned(),
            },
            crate::known_folders::LocationMenuEntry::Rating { stars } => PlaceSummary::Rating {
                name: "レーティング".to_owned(),
                stars,
            },
            crate::known_folders::LocationMenuEntry::Bookshelf => PlaceSummary::Bookshelf {
                name: "本棚フォルダ".to_owned(),
            },
            crate::known_folders::LocationMenuEntry::Separator => PlaceSummary::Separator,
            crate::known_folders::LocationMenuEntry::QuickLocation(location) => {
                remote_folder_place(location.label.to_owned(), location.path)
            }
            crate::known_folders::LocationMenuEntry::DriveRoot(path) => {
                remote_folder_place(path.to_string_lossy().into_owned(), path)
            }
        })
        .collect()
}

fn remote_folder_place(name: String, path: PathBuf) -> PlaceSummary {
    PlaceSummary::Folder {
        entry: remote_entry_from_candidate(CandidateEntry {
            path,
            name,
            kind: RemoteEntryKind::Folder,
            detail: None,
            progress_current: None,
            progress_total: None,
            rating: None,
        }),
    }
}

#[cfg(test)]
fn to_remote_entries(candidates: Vec<CandidateEntry>) -> Vec<RemoteEntry> {
    // The two direct callers intentionally map only small identity fixtures. Production
    // collection paths use `to_remote_entries_bounded` so canonicalization is capped.
    to_remote_entries_bounded(candidates, usize::MAX)
}

fn to_remote_entries_bounded(
    candidates: Vec<CandidateEntry>,
    mapped_limit: usize,
) -> Vec<RemoteEntry> {
    map_bounded_in_order(candidates, mapped_limit, remote_entry_from_candidate)
}

fn map_bounded_in_order<T, U, F>(values: Vec<T>, mapped_limit: usize, map: F) -> Vec<U>
where
    T: Send,
    U: Send,
    F: Fn(T) -> U + Sync + Send,
{
    values
        .into_iter()
        .take(mapped_limit)
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(map)
        .collect()
}

fn remote_entry_from_candidate(candidate: CandidateEntry) -> RemoteEntry {
    let logical = super::path_guard::resolve_existing(candidate.path.to_string_lossy().as_ref())
        .map(|resolved| resolved.logical)
        .unwrap_or_else(|_| candidate.path.clone());
    RemoteEntry {
        path: logical.to_string_lossy().into_owned(),
        name: if candidate.name.trim().is_empty() {
            file_name(&candidate.path)
        } else {
            candidate.name
        },
        kind: candidate.kind,
        thumbnail_address: None,
        detail: candidate.detail,
        progress_current: candidate.progress_current,
        progress_total: candidate.progress_total,
        rating: candidate.rating,
    }
}

fn map_favorite_search_entries(index_entries: Vec<IndexEntry>) -> (Vec<RemoteEntry>, bool) {
    map_favorite_search_entries_with_entry_limit(index_entries, MAX_REMOTE_COLLECTION_ENTRIES)
}

fn map_favorite_search_entries_with_entry_limit(
    index_entries: Vec<IndexEntry>,
    entry_limit: usize,
) -> (Vec<RemoteEntry>, bool) {
    let index_limit_reached = index_entries.len() == SEARCH_RESULT_LIMIT;
    let candidates = index_entries
        .into_iter()
        .filter_map(|entry| {
            let kind = match entry.kind {
                IndexKind::Folder => RemoteEntryKind::Folder,
                IndexKind::ZipFile => RemoteEntryKind::Zip,
                IndexKind::PdfFile => RemoteEntryKind::Pdf,
                IndexKind::VideoFile => return None,
            };
            Some(CandidateEntry {
                path: entry.path,
                name: entry.display_name,
                kind,
                detail: None,
                progress_current: None,
                progress_total: None,
                rating: None,
            })
        })
        .collect();
    let entries = to_remote_entries_bounded(candidates, entry_limit.saturating_add(1));
    let (entries, mapped_truncated) = bound_remote_entries_with_limits(
        entries,
        entry_limit,
        super::REMOTE_LIST_RESPONSE_BUDGET_BYTES,
    );
    (entries, mapped_truncated || index_limit_reached)
}

fn candidate_from_grid_item(
    item: GridItem,
    detail: Option<String>,
    rating: Option<u8>,
) -> Option<CandidateEntry> {
    let name = item.name().into_owned();
    let (path, kind) = match item {
        GridItem::Folder(path) => (path, RemoteEntryKind::Folder),
        GridItem::Image(path) => (path, RemoteEntryKind::Image),
        GridItem::Video(path) => (path, RemoteEntryKind::Video),
        GridItem::Audio(path) => (path, RemoteEntryKind::Audio),
        GridItem::ZipFile(path) => (path, RemoteEntryKind::Zip),
        GridItem::PdfFile(path) => (path, RemoteEntryKind::Pdf),
        GridItem::ConvertibleArchive { path, .. } => (path, RemoteEntryKind::Archive),
        GridItem::ZipImage { zip_path, .. } | GridItem::ZipDir { zip_path, .. } => {
            (zip_path, RemoteEntryKind::Zip)
        }
        GridItem::PdfPage { pdf_path, .. } => (pdf_path, RemoteEntryKind::Pdf),
        GridItem::Stack { representative, .. } => (representative, RemoteEntryKind::Image),
        GridItem::SearchContainer { path, kind, .. } => (
            path,
            match kind {
                crate::grid_item::SearchContainerKind::Folder => RemoteEntryKind::Folder,
                crate::grid_item::SearchContainerKind::Zip => RemoteEntryKind::Zip,
            },
        ),
    };
    Some(CandidateEntry {
        path,
        name,
        kind,
        detail,
        progress_current: None,
        progress_total: None,
        rating,
    })
}

pub(super) fn aggregate_thumb_aspect_height_ratio(settings: &Settings) -> f64 {
    if settings.thumb_aspect_auto {
        1.0
    } else {
        f64::from(settings.thumb_aspect.height_ratio())
    }
}

fn candidate_from_reading_history_entry(
    entry: crate::reading_history_db::ReadingHistoryEntry,
) -> CandidateEntry {
    use crate::reading_history_db::ReadingHistoryKind;

    let (kind, current, total, detail) = match entry.kind {
        ReadingHistoryKind::Folder
        | ReadingHistoryKind::Zip
        | ReadingHistoryKind::Pdf
        | ReadingHistoryKind::Archive => {
            let current = entry.last_page.map(|page| page.max(0) as u64 + 1);
            let total = entry.page_count.map(|count| count.max(0) as u64);
            let kind = match entry.kind {
                ReadingHistoryKind::Folder => RemoteEntryKind::Folder,
                ReadingHistoryKind::Zip => RemoteEntryKind::Zip,
                ReadingHistoryKind::Pdf => RemoteEntryKind::Pdf,
                ReadingHistoryKind::Archive => RemoteEntryKind::Archive,
                ReadingHistoryKind::Video | ReadingHistoryKind::Audio => unreachable!(),
            };
            (kind, current, total, progress_label(current, total))
        }
        ReadingHistoryKind::Video | ReadingHistoryKind::Audio => {
            let current = entry
                .media_position_ms
                .and_then(|value| u64::try_from(value).ok());
            let total = entry
                .media_duration_ms
                .filter(|value| *value > 0)
                .and_then(|value| u64::try_from(value).ok());
            let kind = if entry.kind == ReadingHistoryKind::Audio {
                RemoteEntryKind::Audio
            } else {
                RemoteEntryKind::Video
            };
            (kind, current, total, media_progress_label(current, total))
        }
    };
    CandidateEntry {
        path: entry.path,
        name: entry.title,
        kind,
        detail,
        progress_current: current,
        progress_total: total,
        rating: None,
    }
}

fn progress_label(current: Option<u64>, total: Option<u64>) -> Option<String> {
    match (current, total) {
        (Some(current), Some(total)) if total > 0 => Some(format!("{current} / {total} ページ")),
        (Some(current), _) => Some(format!("{current} ページ")),
        _ => None,
    }
}

fn media_progress_label(current_ms: Option<u64>, total_ms: Option<u64>) -> Option<String> {
    let current = current_ms.map(format_media_time_ms)?;
    Some(match total_ms {
        Some(total) => format!("{current} / {}", format_media_time_ms(total)),
        None => current,
    })
}

fn format_media_time_ms(value_ms: u64) -> String {
    let total_secs = value_ms / 1000;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "項目".to_owned())
}

fn internal_error(context: &str, error: impl std::fmt::Display) -> CollectionError {
    crate::logger::log(format!(
        "remote_ipc: collection error context={context} error={error}"
    ));
    CollectionError::new(CollectionErrorCode::Internal, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::FavoriteEntry;

    fn test_remote_entry(path: impl Into<String>, name: impl Into<String>) -> RemoteEntry {
        RemoteEntry {
            path: path.into(),
            name: name.into(),
            kind: RemoteEntryKind::Image,
            thumbnail_address: None,
            detail: None,
            progress_current: None,
            progress_total: None,
            rating: None,
        }
    }

    fn test_remote_entry_kind(path: &Path, kind: RemoteEntryKind) -> RemoteEntry {
        let mut entry = test_remote_entry(
            path.to_string_lossy().into_owned(),
            path.file_name().unwrap().to_string_lossy().into_owned(),
        );
        entry.kind = kind;
        entry
    }

    fn test_collection_payload(entries: Vec<RemoteEntry>, truncated: bool) -> CollectionPayload {
        let entry_limit = super::super::response_entry_limit(
            MAX_REMOTE_COLLECTION_ENTRIES,
            entries.len(),
            truncated,
        );
        let settings = Settings::default();
        collection_payload(
            &settings,
            "test",
            super::super::remote_grid_sort_state(settings.sort_order, None),
            entries,
            entry_limit,
            truncated,
            CollectionSpreadRequest::default(),
        )
    }

    #[test]
    fn collection_spread_groups_only_images_and_uses_each_parent_catalog_dimensions() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let first_parent = data_dir.path().join("first");
        let second_parent = data_dir.path().join("second");
        std::fs::create_dir_all(&first_parent).unwrap();
        std::fs::create_dir_all(&second_parent).unwrap();
        let cache_dir = crate::catalog::default_cache_dir();
        let first_catalog = crate::catalog::CatalogDb::open(&cache_dir, &first_parent).unwrap();
        first_catalog
            .save("portrait-a.jpg", 1, 10, 8, 8, Some((1200, 1800)), b"a")
            .unwrap();
        first_catalog
            .save("portrait-b.jpg", 1, 10, 8, 8, Some((1200, 1800)), b"b")
            .unwrap();
        drop(first_catalog);
        let second_catalog = crate::catalog::CatalogDb::open(&cache_dir, &second_parent).unwrap();
        second_catalog
            .save("wide.jpg", 1, 10, 8, 8, Some((1800, 1200)), b"wide")
            .unwrap();
        second_catalog
            .save("portrait-c.jpg", 1, 10, 8, 8, Some((1200, 1800)), b"c")
            .unwrap();
        drop(second_catalog);

        let portrait_a = first_parent.join("portrait-a.jpg");
        let video = first_parent.join("clip.mp4");
        let archive = first_parent.join("book.zip");
        let wide = second_parent.join("wide.jpg");
        let portrait_b = first_parent.join("portrait-b.jpg");
        let portrait_c = second_parent.join("portrait-c.jpg");
        let entries = vec![
            test_remote_entry_kind(&portrait_a, RemoteEntryKind::Image),
            test_remote_entry_kind(&video, RemoteEntryKind::Video),
            test_remote_entry_kind(&archive, RemoteEntryKind::Zip),
            test_remote_entry_kind(&wide, RemoteEntryKind::Image),
            test_remote_entry_kind(&portrait_b, RemoteEntryKind::Image),
            test_remote_entry_kind(&portrait_c, RemoteEntryKind::Image),
        ];
        let spread = collection_spread_payload(
            &entries,
            CollectionSpreadRequest {
                spread_mode: Some(RemoteSpreadMode::Ltr),
                reading_direction: Some(RemoteReadingDirection::Ltr),
                force_single_page: false,
            },
        );

        assert_eq!(spread.image_count, 4);
        assert_eq!(spread.groups.len(), 3);
        assert_eq!(
            spread.groups[0].pages,
            [RemoteAddress::file(
                portrait_a.to_string_lossy().into_owned()
            )]
        );
        assert_eq!(
            spread.groups[1].pages,
            [RemoteAddress::file(wide.to_string_lossy().into_owned())]
        );
        assert_eq!(
            spread.groups[2].pages,
            [
                RemoteAddress::file(portrait_b.to_string_lossy().into_owned()),
                RemoteAddress::file(portrait_c.to_string_lossy().into_owned())
            ]
        );
        assert!(spread.groups.iter().all(|group| {
            group.pages.iter().all(|address| {
                !address.path.ends_with("clip.mp4") && !address.path.ends_with("book.zip")
            })
        }));
    }

    #[test]
    fn collection_force_single_page_keeps_one_image_per_group() {
        let entries = (0..5)
            .map(|index| {
                test_remote_entry(
                    format!("C:/collection/page-{index}.jpg"),
                    format!("page-{index}.jpg"),
                )
            })
            .collect::<Vec<_>>();
        let spread = collection_spread_payload(
            &entries,
            CollectionSpreadRequest {
                spread_mode: Some(RemoteSpreadMode::RtlCover),
                reading_direction: Some(RemoteReadingDirection::Rtl),
                force_single_page: true,
            },
        );

        assert_eq!(spread.configured, RemoteSpreadMode::RtlCover);
        assert_eq!(spread.effective, RemoteSpreadMode::Single);
        assert_eq!(spread.groups.len(), entries.len());
        assert!(spread.groups.iter().all(|group| group.pages.len() == 1));
    }

    #[test]
    fn collection_landscape_uses_thumb_only_for_legacy_null_catalog_rows() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let parent = data_dir.path().join("legacy-catalog");
        std::fs::create_dir_all(&parent).unwrap();
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::new(8, 6));
        let mut jpeg = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut jpeg),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        let catalog =
            crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), &parent).unwrap();
        catalog
            .save("legacy-wide.jpg", 1, 10, 8, 6, None, &jpeg)
            .unwrap();
        catalog
            .save(
                "rotated-portrait.jpg",
                1,
                10,
                6,
                8,
                Some((600, 800)),
                b"thumb",
            )
            .unwrap();
        drop(catalog);

        let rotated_path = parent.join("rotated-portrait.jpg");
        let rotation_db = crate::rotation_db::RotationDb::open().unwrap();
        rotation_db
            .set(&rotated_path, crate::rotation_db::Rotation::Cw90)
            .unwrap();
        drop(rotation_db);

        let entries = vec![
            test_remote_entry_kind(&parent.join("legacy-wide.jpg"), RemoteEntryKind::Image),
            test_remote_entry_kind(&parent.join("missing.jpg"), RemoteEntryKind::Image),
            test_remote_entry_kind(&rotated_path, RemoteEntryKind::Image),
        ];

        assert_eq!(
            cached_collection_landscape_flags(&entries),
            [true, false, true]
        );
    }

    #[test]
    /// **横断コレクションでも、カタログが無いフォルダの横長を見つける。**
    ///
    /// 横長判定はコンテナ側と横断コレクション側の 2 か所で作られる。片方だけ直すと、
    /// 同じ本がフォルダから開けば分割され、レーティング一覧から開けば分割されない、
    /// という食い違いになる。
    fn collection_landscape_is_found_without_any_catalog() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let parent = data_dir.path().join("uncached");
        std::fs::create_dir_all(&parent).unwrap();
        let wide = parent.join("wide.png");
        let tall = parent.join("tall.png");
        image::RgbImage::new(300, 150).save(&wide).unwrap();
        image::RgbImage::new(150, 300).save(&tall).unwrap();

        let entries = vec![
            test_remote_entry_kind(&wide, RemoteEntryKind::Image),
            test_remote_entry_kind(&tall, RemoteEntryKind::Image),
        ];
        assert_eq!(cached_collection_landscape_flags(&entries), [true, false]);
    }

    #[test]
    fn collection_page_groups_scale_to_sixty_six_thousand_images() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let parent = data_dir.path().join("large-smart-folder");
        let entries = (0..66_934)
            .map(|index| {
                test_remote_entry(
                    parent
                        .join(format!("page-{index:05}.jpg"))
                        .to_string_lossy()
                        .into_owned(),
                    format!("page-{index:05}.jpg"),
                )
            })
            .collect::<Vec<_>>();
        let started = std::time::Instant::now();
        let spread = collection_spread_payload(
            &entries,
            CollectionSpreadRequest {
                spread_mode: Some(RemoteSpreadMode::Ltr),
                reading_direction: Some(RemoteReadingDirection::Ltr),
                force_single_page: false,
            },
        );
        let elapsed = started.elapsed();

        eprintln!(
            "collection_page_groups images={} groups={} elapsed_ms={:.3}",
            entries.len(),
            spread.groups.len(),
            elapsed.as_secs_f64() * 1000.0
        );
        assert_eq!(spread.image_count, 66_934);
        assert_eq!(spread.groups.len(), 33_467);
    }

    #[test]
    fn reading_history_media_entries_keep_kind_and_time_progress() {
        let mut video = crate::reading_history_db::ReadingHistoryEntry::new(
            PathBuf::from(r"C:\media\clip.mp4"),
            crate::reading_history_db::ReadingHistoryKind::Video,
            None,
            "clip".to_owned(),
            None,
            None,
        );
        video.media_position_ms = Some(65_000);
        video.media_duration_ms = Some(3_665_000);
        let video = candidate_from_reading_history_entry(video);
        assert_eq!(video.kind, RemoteEntryKind::Video);
        assert_eq!(video.progress_current, Some(65_000));
        assert_eq!(video.progress_total, Some(3_665_000));
        assert_eq!(video.detail.as_deref(), Some("1:05 / 1:01:05"));

        let audio = candidate_from_reading_history_entry(
            crate::reading_history_db::ReadingHistoryEntry::new(
                PathBuf::from(r"C:\media\track.flac"),
                crate::reading_history_db::ReadingHistoryKind::Audio,
                None,
                "track".to_owned(),
                None,
                None,
            ),
        );
        assert_eq!(audio.kind, RemoteEntryKind::Audio);
        assert_eq!(audio.progress_current, None);
        assert_eq!(audio.progress_total, None);
        assert_eq!(audio.detail, None);
    }

    #[test]
    fn hidden_location_settings_are_reflected() {
        let mut settings = Settings::default();
        settings.show_location_drive_list = false;
        settings.show_location_reading_history = false;
        settings.show_location_rating = false;
        settings.show_location_bookshelf = true;
        settings.show_location_desktop = false;
        settings.show_location_pictures = false;
        settings.show_location_downloads = false;
        settings.show_location_drive_roots = false;
        let places = visible_places(&settings);
        assert_eq!(
            places,
            [
                PlaceSummary::Bookmarks {
                    name: "ブックマーク".to_owned(),
                },
                PlaceSummary::Bookshelf {
                    name: "本棚フォルダ".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn drive_list_and_direct_places_produce_openable_folders() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("drive-a");
        let second = temp.path().join("drive-b");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let settings = Settings::default();
        let engine = CollectionEngine::new(settings.clone());
        let payload = engine
            .drive_list_from_paths(
                &settings,
                settings.sort_order,
                vec![first.clone(), second.clone()],
                CollectionSpreadRequest::default(),
            )
            .unwrap();
        assert_eq!(payload.title, "ドライブ一覧");
        assert_eq!(payload.entries.len(), 2);
        for entry in &payload.entries {
            assert_eq!(entry.kind, RemoteEntryKind::Folder);
            assert!(super::super::path_guard::resolve_existing(&entry.path).is_ok());
        }

        let place = remote_folder_place("デスクトップ".to_owned(), first);
        let PlaceSummary::Folder { entry } = place else {
            panic!("direct place must stay a typed folder entry");
        };
        assert_eq!(entry.name, "デスクトップ");
        assert_eq!(entry.kind, RemoteEntryKind::Folder);
        assert!(super::super::path_guard::resolve_existing(&entry.path).is_ok());
    }

    #[test]
    fn entries_outside_favorites_are_kept_with_absolute_addresses() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let inside = root.join("album/page.jpg");
        let outside = temp.path().join("outside.jpg");
        std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
        std::fs::write(&inside, b"inside").unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        let candidates = vec![
            CandidateEntry {
                path: inside.clone(),
                name: "page.jpg".to_owned(),
                kind: RemoteEntryKind::Image,
                detail: None,
                progress_current: None,
                progress_total: None,
                rating: None,
            },
            CandidateEntry {
                path: outside.clone(),
                name: "outside.jpg".to_owned(),
                kind: RemoteEntryKind::Image,
                detail: None,
                progress_current: None,
                progress_total: None,
                rating: None,
            },
        ];
        let entries = to_remote_entries(candidates);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            PathBuf::from(&entries[0].path),
            super::super::path_guard::resolve_existing(inside.to_string_lossy().as_ref())
                .unwrap()
                .logical
        );
        assert_eq!(
            PathBuf::from(&entries[1].path),
            super::super::path_guard::resolve_existing(outside.to_string_lossy().as_ref())
                .unwrap()
                .logical
        );
        assert!(
            entries
                .iter()
                .all(|entry| Path::new(&entry.path).is_absolute())
        );
        let json = serde_json::to_value(&entries).unwrap();
        assert_eq!(json[0]["path"].as_str(), Some(entries[0].path.as_str()));
        assert_eq!(json[1]["path"].as_str(), Some(entries[1].path.as_str()));
    }

    #[test]
    fn every_non_favorite_collection_source_maps_to_an_openable_absolute_address() {
        let temp = tempfile::tempdir().unwrap();
        let rating = temp.path().join("rated.zip");
        let media_bookmark = temp.path().join("clip.mp4");
        let book_bookmark = temp.path().join("bookmark.pdf");
        let history = temp.path().join("recent.pdf");
        let books_root = temp.path().join("books");
        let book = books_root.join("volume");
        let smart_root = temp.path().join("smart");
        let smart_item = smart_root.join("nested/page.jpg");
        for path in [&rating, &media_bookmark, &book_bookmark, &history] {
            std::fs::write(path, b"fixture").unwrap();
        }
        std::fs::create_dir_all(&book).unwrap();
        std::fs::create_dir_all(smart_item.parent().unwrap()).unwrap();
        std::fs::write(&smart_item, b"page").unwrap();
        let candidates = [
            (&rating, RemoteEntryKind::Zip, "rated.zip"),
            (&media_bookmark, RemoteEntryKind::Video, "clip.mp4"),
            (&book_bookmark, RemoteEntryKind::Pdf, "bookmark.pdf"),
            (&history, RemoteEntryKind::Pdf, "recent.pdf"),
            (&book, RemoteEntryKind::Folder, "volume"),
            (&smart_item, RemoteEntryKind::Image, "page.jpg"),
        ]
        .into_iter()
        .map(|(path, kind, name)| CandidateEntry {
            path: path.clone(),
            name: name.to_owned(),
            kind,
            detail: None,
            progress_current: None,
            progress_total: None,
            rating: None,
        })
        .collect();

        let entries = to_remote_entries(candidates);

        assert_eq!(entries.len(), 6);
        for entry in &entries {
            assert!(
                super::super::path_guard::resolve_existing(&entry.path).is_ok(),
                "collection entry must resolve: {}",
                entry.name
            );
            assert!(Path::new(&entry.path).is_absolute());
        }
        let json = serde_json::to_value(&entries).unwrap();
        assert_eq!(
            json.as_array().unwrap().len(),
            entries.len(),
            "every mapped source must remain serialized"
        );
        for (encoded, entry) in json.as_array().unwrap().iter().zip(&entries) {
            assert_eq!(encoded["path"].as_str(), Some(entry.path.as_str()));
        }
    }

    #[test]
    fn parallel_mapping_matches_sequential_order_and_content_for_real_paths() {
        let temp = tempfile::tempdir().unwrap();
        let candidates = (0..32)
            .map(|index| {
                let path = temp.path().join(format!("entry-{index:02}.jpg"));
                std::fs::write(&path, format!("fixture-{index}")).unwrap();
                CandidateEntry {
                    path,
                    name: format!("entry-{index:02}"),
                    kind: RemoteEntryKind::Image,
                    detail: Some(format!("detail-{index}")),
                    progress_current: Some(index),
                    progress_total: Some(32),
                    rating: Some((index % 5 + 1) as u8),
                }
            })
            .collect::<Vec<_>>();
        let expected = candidates
            .clone()
            .into_iter()
            .map(remote_entry_from_candidate)
            .collect::<Vec<_>>();

        let actual = to_remote_entries_bounded(candidates, 32);

        assert_eq!(actual, expected);
    }

    #[test]
    fn bounded_parallel_mapping_invokes_the_mapper_only_for_the_taken_prefix() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let mapped = map_bounded_in_order((0..20).collect(), 7, |value| {
            calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            value * 2
        });

        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 7);
        assert_eq!(mapped, vec![0, 2, 4, 6, 8, 10, 12]);
    }

    #[test]
    fn aggregate_payload_count_limit_is_injectable_without_a_large_fixture() {
        let test_limit = 7;
        let entries = (0..=test_limit)
            .map(|index| {
                test_remote_entry(
                    format!("C:/entries/entry-{index}"),
                    format!("entry-{index}"),
                )
            })
            .collect();
        let (entries, truncated) =
            bound_remote_entries_with_limits(entries, test_limit, usize::MAX);
        assert_eq!(entries.len(), test_limit);
        assert!(truncated);
    }

    #[test]
    fn collection_accepts_one_hundred_thousand_short_entries_and_truncates_the_next() {
        let short = test_remote_entry("C:/p", "p");
        let (entries, truncated) =
            bound_remote_entries(vec![short.clone(); MAX_REMOTE_COLLECTION_ENTRIES]);
        assert_eq!(entries.len(), MAX_REMOTE_COLLECTION_ENTRIES);
        assert!(!truncated);

        let (entries, truncated) =
            bound_remote_entries(vec![short; MAX_REMOTE_COLLECTION_ENTRIES + 1]);
        assert_eq!(entries.len(), MAX_REMOTE_COLLECTION_ENTRIES);
        assert!(truncated);
    }

    #[test]
    fn collection_long_entries_stay_below_the_ipc_frame_limit() {
        let long_name = "x".repeat(400);
        let candidates = (0..MAX_REMOTE_COLLECTION_ENTRIES)
            .map(|index| {
                test_remote_entry(
                    format!("C:/{long_name}/{index:06}.jpg"),
                    format!("{long_name}-{index:06}.jpg"),
                )
            })
            .collect();
        let (entries, truncated) = bound_remote_entries(candidates);
        let payload = test_collection_payload(entries, truncated);

        assert!(payload.truncated);
        assert_eq!(payload.entry_limit, payload.entries.len());
        let response = CollectionResponse::Success(payload);
        assert!(
            serde_json::to_vec(&response).unwrap().len()
                < mimageviewer_ipc::MAX_RESPONSE_FRAME_BYTES
        );
    }

    #[test]
    fn aggregate_ignore_hides_other_archive_bookmark_candidates() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("bookmark.7z");
        let image = temp.path().join("page.jpg");
        std::fs::write(&archive, b"archive").unwrap();
        std::fs::write(&image, b"image").unwrap();
        let candidates = || {
            vec![
                CandidateEntry {
                    path: archive.clone(),
                    name: "bookmark.7z".to_owned(),
                    kind: RemoteEntryKind::Archive,
                    detail: None,
                    progress_current: Some(1),
                    progress_total: None,
                    rating: None,
                },
                CandidateEntry {
                    path: image.clone(),
                    name: "page.jpg".to_owned(),
                    kind: RemoteEntryKind::Image,
                    detail: None,
                    progress_current: None,
                    progress_total: None,
                    rating: None,
                },
            ]
        };
        let mut settings = Settings::default();
        settings.set_archive_file_handling(crate::settings::ArchiveFileHandling::Ignore);
        let engine = CollectionEngine::new(settings.clone());
        let ignored = engine
            .payload(
                &settings,
                "bookmarks",
                candidates(),
                settings.sort_order,
                None,
                CollectionSpreadRequest::default(),
            )
            .unwrap();
        assert_eq!(ignored.entries.len(), 1);
        assert_eq!(ignored.entries[0].kind, RemoteEntryKind::Image);

        settings.set_archive_file_handling(crate::settings::ArchiveFileHandling::Ask);
        let included = engine
            .payload(
                &settings,
                "bookmarks",
                candidates(),
                settings.sort_order,
                None,
                CollectionSpreadRequest::default(),
            )
            .unwrap();
        assert_eq!(included.entries.len(), 2);
        assert!(
            included
                .entries
                .iter()
                .any(|entry| entry.kind == RemoteEntryKind::Archive)
        );
    }

    #[test]
    fn aggregate_payload_assigns_same_folder_sidecar_without_removing_it() {
        let temp = tempfile::tempdir().unwrap();
        let video = temp.path().join("clip.mp4");
        let sidecar = temp.path().join("clip.jpg");
        std::fs::write(&video, b"video").unwrap();
        std::fs::write(&sidecar, b"image").unwrap();
        let candidates = vec![
            CandidateEntry {
                path: video,
                name: "clip.mp4".to_owned(),
                kind: RemoteEntryKind::Video,
                detail: None,
                progress_current: None,
                progress_total: None,
                rating: None,
            },
            CandidateEntry {
                path: sidecar,
                name: "clip.jpg".to_owned(),
                kind: RemoteEntryKind::Image,
                detail: None,
                progress_current: None,
                progress_total: None,
                rating: None,
            },
        ];
        let settings = Settings::default();
        let engine = CollectionEngine::new(settings.clone());

        let payload = engine
            .payload(
                &settings,
                "aggregate",
                candidates,
                settings.sort_order,
                None,
                CollectionSpreadRequest::default(),
            )
            .unwrap();

        assert_eq!(payload.entries.len(), 2);
        let video = payload
            .entries
            .iter()
            .find(|entry| entry.kind == RemoteEntryKind::Video)
            .unwrap();
        assert_eq!(
            video
                .thumbnail_address
                .as_ref()
                .map(|address| PathBuf::from(&address.path)),
            Some(temp.path().join("clip.jpg"))
        );
        let image = payload
            .entries
            .iter()
            .find(|entry| entry.kind == RemoteEntryKind::Image)
            .unwrap();
        assert!(image.thumbnail_address.is_none());
    }

    #[test]
    fn aggregate_payload_leaves_video_source_empty_without_sidecar() {
        let temp = tempfile::tempdir().unwrap();
        let video = temp.path().join("clip.mp4");
        std::fs::write(&video, b"video").unwrap();
        let settings = Settings::default();
        let entries = vec![RemoteEntry {
            path: video.to_string_lossy().into_owned(),
            name: "clip.mp4".to_owned(),
            kind: RemoteEntryKind::Video,
            thumbnail_address: None,
            detail: None,
            progress_current: None,
            progress_total: None,
            rating: None,
        }];

        let sources = super::super::RemoteThumbnailSources::for_remote_entries(&settings, &entries);

        assert!(
            sources
                .source_address(&video, RemoteEntryKind::Video)
                .is_none()
        );
    }

    fn favorite_with_container_index(path: PathBuf, enabled: bool) -> FavoriteEntry {
        let mut favorite = FavoriteEntry::new("favorite".to_owned(), path);
        favorite.auto_index_structure = enabled;
        favorite
    }

    fn search_engine(favorite: FavoriteEntry) -> CollectionEngine {
        CollectionEngine::new(Settings {
            favorites: vec![favorite],
            ..Default::default()
        })
    }

    fn search_request(query: impl Into<String>) -> FavoriteSearchRequest {
        FavoriteSearchRequest {
            query: query.into(),
            kind: FavoriteSearchKind::All,
        }
    }

    fn search_success(response: FavoriteSearchResponse) -> FavoriteSearchPayload {
        match response {
            FavoriteSearchResponse::Success(payload) => payload,
            FavoriteSearchResponse::Error(error) => panic!("unexpected search error: {error:?}"),
        }
    }

    fn tag_engine(root: PathBuf) -> CollectionEngine {
        CollectionEngine::new(Settings {
            favorites: vec![FavoriteEntry::new("favorite".to_owned(), root)],
            ..Default::default()
        })
    }

    fn tag_request(tag: impl Into<String>) -> TagItemsRequest {
        TagItemsRequest {
            tag: tag.into(),
            kind: TagItemKind::All,
        }
    }

    fn tag_browse_success(response: TagBrowseResponse) -> TagBrowsePayload {
        match response {
            TagBrowseResponse::Success(payload) => payload,
            TagBrowseResponse::Error(error) => panic!("unexpected tag browse error: {error:?}"),
        }
    }

    fn tag_items_success(response: TagItemsResponse) -> TagItemsPayload {
        match response {
            TagItemsResponse::Success(payload) => payload,
            TagItemsResponse::Error(error) => panic!("unexpected tag items error: {error:?}"),
        }
    }

    fn collection_success(response: CollectionResponse) -> CollectionPayload {
        match response {
            CollectionResponse::Success(payload) => payload,
            CollectionResponse::Error(error) => panic!("unexpected collection error: {error:?}"),
        }
    }

    #[test]
    fn missing_tags_db_is_unavailable_and_is_not_created() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let root = data_dir.path().join("favorite");
        std::fs::create_dir(&root).unwrap();
        let db_path = crate::tags_db::TagsDb::db_path();
        assert!(!db_path.exists());

        let payload = tag_browse_success(tag_engine(root).tag_browse(TagBrowseRequest));

        assert_eq!(payload.state, TagIndexState::Unavailable);
        assert!(payload.all.is_empty());
        assert!(!db_path.exists());
    }

    #[test]
    fn empty_tags_db_reports_empty_for_browse_and_items() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let root = data_dir.path().join("favorite");
        std::fs::create_dir(&root).unwrap();
        drop(crate::tags_db::TagsDb::open_at(&crate::tags_db::TagsDb::db_path()).unwrap());
        let engine = tag_engine(root);

        let browse = tag_browse_success(engine.tag_browse(TagBrowseRequest));
        let items = tag_items_success(engine.tag_items(tag_request("cat")));

        assert_eq!(browse.state, TagIndexState::Empty);
        assert_eq!(items.state, TagIndexState::Empty);
        assert!(items.listing.entries.is_empty());
    }

    #[test]
    fn tag_items_reject_empty_and_overlong_queries() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let root = data_dir.path().join("favorite");
        std::fs::create_dir(&root).unwrap();
        let engine = tag_engine(root);
        for tag in [String::new(), "   ".to_owned(), "あ".repeat(201)] {
            let TagItemsResponse::Error(error) = engine.tag_items(tag_request(tag)) else {
                panic!("invalid tag unexpectedly succeeded");
            };
            assert_eq!(error.code, CollectionErrorCode::BadRequest);
        }
    }

    #[test]
    fn tag_items_include_existing_paths_outside_favorites_and_keep_missing_rows_unchanged() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let root = data_dir.path().join("favorite");
        let inside = root.join("inside.jpg");
        let outside = data_dir.path().join("outside.jpg");
        let missing = root.join("missing.jpg");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&inside, b"inside").unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        let missing_key = crate::tags_db::item_key_for_path(&missing);
        let mut db = crate::tags_db::TagsDb::open_at(&crate::tags_db::TagsDb::db_path()).unwrap();
        for path in [&inside, &outside, &missing] {
            db.set_item_tags(&crate::tags_db::item_key_for_path(path), ["cat"], "test")
                .unwrap();
        }
        drop(db);

        let payload = tag_items_success(tag_engine(root).tag_items(tag_request("cat")));

        assert_eq!(payload.state, TagIndexState::Ready);
        assert_eq!(payload.listing.entries.len(), 2);
        let inside_entry = payload
            .listing
            .entries
            .iter()
            .find(|entry| {
                PathBuf::from(&entry.path)
                    == super::super::path_guard::resolve_existing(inside.to_string_lossy().as_ref())
                        .unwrap()
                        .logical
            })
            .unwrap();
        assert!(Path::new(&inside_entry.path).is_absolute());
        let outside_entry = payload
            .listing
            .entries
            .iter()
            .find(|entry| {
                PathBuf::from(&entry.path)
                    == super::super::path_guard::resolve_existing(
                        outside.to_string_lossy().as_ref(),
                    )
                    .unwrap()
                    .logical
            })
            .unwrap();
        assert!(Path::new(&outside_entry.path).is_absolute());
        let json = serde_json::to_value(&payload).unwrap();
        assert!(
            json["listing"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"].as_str() == Some(outside_entry.path.as_str()))
        );
        let db = crate::tags_db::TagsDb::open_readonly(&crate::tags_db::TagsDb::db_path()).unwrap();
        assert_eq!(db.display_tags_for_item(&missing_key), vec!["#cat"]);
        assert!(db.has_item_state(&missing_key));
    }

    #[test]
    fn live_collection_settings_and_absolute_paths_change_together() {
        let data_dir = crate::settings_db::DataDirOverrideGuard::new();
        let first_root = data_dir.path().join("books-first");
        let second_root = data_dir.path().join("books-second");
        std::fs::create_dir_all(first_root.join("First")).unwrap();
        std::fs::create_dir_all(second_root.join("Second")).unwrap();
        let mut first_smart = crate::settings::SmartFolderDefinition::new("First smart");
        first_smart
            .rules
            .push(crate::settings::SmartFolderRule::new(
                data_dir.path().join("smart-first"),
                false,
                Default::default(),
            ));
        let mut settings = Settings {
            book_root: Some(first_root.clone()),
            smart_folders: vec![first_smart],
            ..Default::default()
        };
        let db = crate::settings_db::SettingsDb::create_new(data_dir.path()).unwrap();
        db.save_full(&settings).unwrap();
        let favorites =
            super::super::live_favorites::LiveFavorites::live(settings.favorites.clone()).unwrap();
        let engine = CollectionEngine::new_with_live_favorites(settings.clone(), favorites);

        let first = collection_success(engine.collection(CollectionRequest {
            kind: CollectionKind::Bookshelf,
            spread_mode: None,
            reading_direction: None,
            force_single_page: false,
        }));
        assert_eq!(first.entries.len(), 1);
        assert_eq!(first.entries[0].name, "First");
        assert_eq!(
            PathBuf::from(&first.entries[0].path),
            super::super::path_guard::resolve_existing(
                first_root.join("First").to_string_lossy().as_ref(),
            )
            .unwrap()
            .logical
        );
        let HomeResponse::Success(first_home) = engine.home() else {
            panic!("initial home failed");
        };
        assert_eq!(first_home.smart_folders[0].name, "First smart");

        let mut second_smart = crate::settings::SmartFolderDefinition::new("Second smart");
        second_smart
            .rules
            .push(crate::settings::SmartFolderRule::new(
                data_dir.path().join("smart-second"),
                true,
                Default::default(),
            ));
        settings.book_root = Some(second_root.clone());
        settings.smart_folders = vec![second_smart];
        db.save_full(&settings).unwrap();

        let second = collection_success(engine.collection(CollectionRequest {
            kind: CollectionKind::Bookshelf,
            spread_mode: None,
            reading_direction: None,
            force_single_page: false,
        }));
        assert_eq!(second.entries.len(), 1);
        assert_eq!(second.entries[0].name, "Second");
        assert_eq!(
            PathBuf::from(&second.entries[0].path),
            super::super::path_guard::resolve_existing(
                second_root.join("Second").to_string_lossy().as_ref(),
            )
            .unwrap()
            .logical
        );
        let HomeResponse::Success(second_home) = engine.home() else {
            panic!("refreshed home failed");
        };
        assert_eq!(second_home.smart_folders[0].name, "Second smart");
    }

    #[test]
    fn tag_items_map_unknown_files_to_other_and_only_real_directories_to_folder() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let root = data_dir.path().join("favorite");
        let unknown = root.join("notes.unknown-extension");
        let folder = root.join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(&unknown, b"unknown").unwrap();
        let mut db = crate::tags_db::TagsDb::open_at(&crate::tags_db::TagsDb::db_path()).unwrap();
        for path in [&unknown, &folder] {
            db.set_item_tags(&crate::tags_db::item_key_for_path(path), ["cat"], "test")
                .unwrap();
        }
        drop(db);
        let engine = tag_engine(root);

        let all = tag_items_success(engine.tag_items(tag_request("cat")));
        assert_eq!(all.listing.entries.len(), 2);
        assert_eq!(
            all.listing
                .entries
                .iter()
                .find(|entry| entry.path.ends_with("notes.unknown-extension"))
                .map(|entry| entry.kind),
            Some(RemoteEntryKind::Other)
        );
        let folders = tag_items_success(engine.tag_items(TagItemsRequest {
            tag: "cat".to_owned(),
            kind: TagItemKind::Folder,
        }));
        assert_eq!(folders.listing.entries.len(), 1);
        assert!(folders.listing.entries[0].path.ends_with("folder"));
        assert_eq!(folders.listing.entries[0].kind, RemoteEntryKind::Folder);
    }

    #[test]
    fn tag_item_mapping_stops_at_one_over_the_response_limit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let image = root.join("image.jpg");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&image, b"image").unwrap();
        let item_key = crate::tags_db::item_key_for_path(&image);
        let test_limit = 7;
        let item_keys = vec![item_key; test_limit + 1];

        let (entries, truncated) = map_tag_item_keys_with_entry_limit(
            item_keys,
            TagItemKind::All,
            crate::tag_view::TAG_VIEW_RESULT_LIMIT,
            true,
            test_limit,
        );

        assert_eq!(entries.len(), test_limit);
        assert!(truncated);
    }

    #[test]
    fn tag_item_mapping_honors_archive_ignore_without_an_extra_settings_read() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("tagged.7z");
        std::fs::write(&archive, b"archive").unwrap();
        let key = crate::tags_db::item_key_for_path(&archive);

        let (ignored, _) = map_tag_item_keys(
            vec![key.clone()],
            TagItemKind::All,
            crate::tag_view::TAG_VIEW_RESULT_LIMIT,
            false,
        );
        let (included, _) = map_tag_item_keys(
            vec![key],
            TagItemKind::All,
            crate::tag_view::TAG_VIEW_RESULT_LIMIT,
            true,
        );

        assert!(ignored.is_empty());
        assert_eq!(included.len(), 1);
        assert_eq!(included[0].kind, RemoteEntryKind::Archive);
    }

    #[test]
    fn tag_item_kind_filter_scans_past_the_first_response_window() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        std::fs::create_dir_all(&root).unwrap();
        let test_limit = 3;
        let mut item_keys = Vec::new();
        for index in 0..=test_limit {
            let path = root.join(format!("{index:04}.mp4"));
            std::fs::write(&path, b"video").unwrap();
            item_keys.push(crate::tags_db::item_key_for_path(&path));
        }
        let image = root.join("zzzz.jpg");
        std::fs::write(&image, b"image").unwrap();
        item_keys.push(crate::tags_db::item_key_for_path(&image));
        let (entries, truncated) = map_tag_item_keys_with_entry_limit(
            item_keys,
            TagItemKind::Image,
            crate::tag_view::TAG_VIEW_FILTERED_KEY_SCAN_LIMIT,
            true,
            test_limit,
        );

        assert_eq!(entries.len(), 1);
        assert!(entries[0].path.ends_with("zzzz.jpg"));
        assert_eq!(entries[0].kind, RemoteEntryKind::Image);
        assert!(!truncated);
    }

    #[test]
    fn tag_browse_bounds_the_name_ordered_all_list() {
        let summaries = (0..=MAX_REMOTE_TAG_CHOICES)
            .map(|index| crate::tags_db::TagSummary {
                tag: format!("tag-{index:04}"),
                tag_key: format!("tag-{index:04}"),
                count: index,
                last_applied_at: index as i64,
            })
            .collect();

        let payload = tag_browse_payload_from_summaries(summaries, &[]);

        assert_eq!(payload.state, TagIndexState::Ready);
        assert_eq!(payload.all.len(), MAX_REMOTE_TAG_CHOICES);
        assert!(payload.all_truncated);
        assert_eq!(
            payload.all.first().map(|choice| choice.name.as_str()),
            Some("tag-0000")
        );
        assert_eq!(
            payload.all.last().map(|choice| choice.name.as_str()),
            Some("tag-1999")
        );
    }

    #[test]
    fn favorite_search_rejects_empty_and_overlong_queries() {
        let root = tempfile::tempdir().unwrap();
        let engine = search_engine(favorite_with_container_index(
            root.path().to_path_buf(),
            true,
        ));
        for query in [String::new(), "   ".to_owned(), "あ".repeat(201)] {
            let FavoriteSearchResponse::Error(error) =
                engine.favorite_search(search_request(query))
            else {
                panic!("invalid query unexpectedly succeeded");
            };
            assert_eq!(error.code, CollectionErrorCode::BadRequest);
        }
    }

    #[test]
    fn favorite_search_is_disabled_without_a_container_index_favorite() {
        let root = tempfile::tempdir().unwrap();
        let engine = search_engine(favorite_with_container_index(
            root.path().to_path_buf(),
            false,
        ));

        let payload = search_success(engine.favorite_search(search_request("album")));

        assert_eq!(payload.index_state, FavoriteSearchIndexState::Disabled);
        assert!(payload.listing.entries.is_empty());
    }

    #[test]
    fn missing_search_index_is_unavailable_and_is_not_created() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let root = data_dir.path().join("favorite");
        std::fs::create_dir(&root).unwrap();
        let engine = search_engine(favorite_with_container_index(root, true));
        let db_path = SearchIndexDb::db_path();
        assert!(!db_path.exists());

        let payload = search_success(engine.favorite_search(search_request("album")));

        assert_eq!(payload.index_state, FavoriteSearchIndexState::Unavailable);
        assert!(payload.listing.entries.is_empty());
        assert!(!db_path.exists());
    }

    #[test]
    fn favorite_search_keeps_index_paths_outside_favorites() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let root = data_dir.path().join("favorite");
        let inside = root.join("match-inside");
        let outside = data_dir.path().join("match-outside");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let favorite = favorite_with_container_index(root.clone(), true);
        let db = SearchIndexDb::open_at(&SearchIndexDb::db_path()).unwrap();
        db.upsert_children(
            &root,
            &root,
            &[
                IndexEntry {
                    path: inside,
                    display_name: "match-inside".to_owned(),
                    kind: IndexKind::Folder,
                    mtime: 0,
                },
                IndexEntry {
                    path: outside,
                    display_name: "match-outside".to_owned(),
                    kind: IndexKind::Folder,
                    mtime: 0,
                },
            ],
        )
        .unwrap();
        drop(db);

        let payload = search_success(
            search_engine(favorite.clone()).favorite_search(search_request("match")),
        );

        assert_eq!(payload.index_state, FavoriteSearchIndexState::Ready);
        assert_eq!(payload.listing.entries.len(), 2);
        assert!(
            payload
                .listing
                .entries
                .iter()
                .all(|entry| Path::new(&entry.path).is_absolute())
        );
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(
            json["listing"]["entries"].as_array().unwrap().len(),
            payload.listing.entries.len()
        );
        for entry in &payload.listing.entries {
            assert!(
                json["listing"]["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|encoded| encoded["path"].as_str() == Some(entry.path.as_str()))
            );
        }
    }

    #[test]
    fn favorite_search_mapping_stops_at_one_over_the_response_limit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let target = root.join("album");
        std::fs::create_dir_all(&target).unwrap();
        let test_limit = 7;
        let index_entries = (0..=test_limit)
            .map(|index| IndexEntry {
                path: target.clone(),
                display_name: format!("album-{index}"),
                kind: IndexKind::Folder,
                mtime: 0,
            })
            .collect();

        let (entries, truncated) =
            map_favorite_search_entries_with_entry_limit(index_entries, test_limit);

        assert_eq!(entries.len(), test_limit);
        assert!(truncated);
    }
}
