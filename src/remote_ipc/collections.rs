use std::path::{Path, PathBuf};
use std::sync::Arc;

use mimageviewer_ipc::{
    CollectionError, CollectionErrorCode, CollectionKind, CollectionPayload, CollectionRequest,
    CollectionResponse, FavoriteSearchIndexState, FavoriteSearchKind, FavoriteSearchPayload,
    FavoriteSearchRequest, FavoriteSearchResponse, HomePayload, HomeResponse, PlaceKind,
    PlaceSummary, RemoteEntry, RemoteEntryKind, RemoteTagChoice, SmartFolderSummary,
    TagBrowsePayload, TagBrowseRequest, TagBrowseResponse, TagIndexState, TagItemKind,
    TagItemsPayload, TagItemsRequest, TagItemsResponse,
};

use crate::grid_item::GridItem;
use crate::search_index_db::{IndexEntry, IndexKind, SEARCH_RESULT_LIMIT, SearchIndexDb};
use crate::settings::{FavoriteEntry, Settings};

use super::path_guard::{
    ResolvedFavoriteRoot, map_existing_to_resolved_favorite, resolve_existing_favorite_roots,
};

const MAX_REMOTE_COLLECTION_ENTRIES: usize = 1000;
const MAX_REMOTE_TAG_CHOICES: usize = 2000;

pub(super) struct CollectionEngine {
    settings: Settings,
    sort_settings: super::RemoteSortSettingsSource,
    favorite_roots: Arc<super::live_favorites::RemoteFavoriteRoots>,
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

impl CollectionEngine {
    pub(super) fn new(settings: Settings) -> Self {
        let favorite_roots =
            super::live_favorites::RemoteFavoriteRoots::snapshot(settings.favorites.clone());
        Self::new_with_favorite_roots(settings, favorite_roots)
    }

    pub(super) fn new_with_favorite_roots(
        settings: Settings,
        favorite_roots: Arc<super::live_favorites::RemoteFavoriteRoots>,
    ) -> Self {
        Self {
            sort_settings: super::RemoteSortSettingsSource::Snapshot(settings.sort_order),
            settings,
            favorite_roots,
        }
    }

    pub(super) fn new_with_live_favorite_roots(
        settings: Settings,
        favorite_roots: Arc<super::live_favorites::RemoteFavoriteRoots>,
    ) -> Self {
        Self {
            settings,
            sort_settings: super::RemoteSortSettingsSource::Live,
            favorite_roots,
        }
    }

    pub(super) fn home(&self) -> HomeResponse {
        HomeResponse::Success(HomePayload {
            smart_folders: self
                .settings
                .smart_folders
                .iter()
                .map(|definition| SmartFolderSummary {
                    id: definition.id.to_string(),
                    name: definition.name.clone(),
                })
                .collect(),
            places: visible_places(&self.settings),
        })
    }

    pub(super) fn collection(&self, request: CollectionRequest) -> CollectionResponse {
        let sort_order = match self.sort_settings.load() {
            Ok(sort_order) => sort_order,
            Err(error) => {
                return CollectionResponse::Error(internal_error(
                    "最新の並び順を読み込めませんでした",
                    error,
                ));
            }
        };
        let result = match request.kind {
            CollectionKind::ReadingHistory => self.reading_history(sort_order),
            CollectionKind::Rating { stars } => self.rating(stars, sort_order),
            CollectionKind::Bookshelf => self.bookshelf(sort_order),
            CollectionKind::Bookmarks => self.bookmarks(sort_order),
            CollectionKind::SmartFolder { definition_id } => {
                self.smart_folder(&definition_id, sort_order)
            }
        };
        match result {
            Ok(payload) => CollectionResponse::Success(payload),
            Err(error) => CollectionResponse::Error(error),
        }
    }

    /// Favorite search stays in CollectionEngine because it must share the collection
    /// allowlist mapping, response bound, aspect ratio, and fixed-sort contract.
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
        let sort_order = self
            .sort_settings
            .load()
            .map_err(|error| internal_error("最新の並び順を読み込めませんでした", error))?;
        let favorites = self.favorite_roots.current().map_err(|error| {
            crate::logger::log(format!("remote_ipc: {error}"));
            CollectionError::new(
                CollectionErrorCode::Internal,
                "最新のお気に入りを読み込めませんでした",
            )
        })?;
        if !favorites
            .iter()
            .any(|favorite| favorite.auto_index_structure)
        {
            return Ok(
                self.favorite_search_state_payload(sort_order, FavoriteSearchIndexState::Disabled)
            );
        }

        let db = match SearchIndexDb::open_readonly() {
            Ok(db) => db,
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: favorite search index open failed: {error}"
                ));
                return Ok(self.favorite_search_state_payload(
                    sort_order,
                    FavoriteSearchIndexState::Unavailable,
                ));
            }
        };
        let roots = favorites
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
            &roots,
            kind,
            crate::search_query::MatchMode::And,
        ) {
            Ok(entries) => entries,
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: favorite search index read failed: {error}"
                ));
                return Ok(self.favorite_search_state_payload(
                    sort_order,
                    FavoriteSearchIndexState::Unavailable,
                ));
            }
        };
        let (entries, truncated) = map_favorite_search_entries(&favorites, index_entries);
        Ok(FavoriteSearchPayload {
            listing: self.favorite_search_listing(sort_order, entries, truncated),
            index_state: FavoriteSearchIndexState::Ready,
        })
    }

    fn favorite_search_state_payload(
        &self,
        sort_order: crate::settings::SortOrder,
        index_state: FavoriteSearchIndexState,
    ) -> FavoriteSearchPayload {
        FavoriteSearchPayload {
            listing: self.favorite_search_listing(sort_order, Vec::new(), false),
            index_state,
        }
    }

    fn favorite_search_listing(
        &self,
        sort_order: crate::settings::SortOrder,
        entries: Vec<RemoteEntry>,
        truncated: bool,
    ) -> CollectionPayload {
        CollectionPayload {
            title: "検索結果".to_owned(),
            thumb_aspect_height_ratio: aggregate_thumb_aspect_height_ratio(&self.settings),
            sort_state: super::remote_grid_sort_state(
                sort_order,
                Some(super::FIXED_LIST_SORT_LOCK_REASON),
            ),
            entries,
            entry_limit: MAX_REMOTE_COLLECTION_ENTRIES,
            truncated,
        }
    }

    pub(super) fn tag_browse(&self, _request: TagBrowseRequest) -> TagBrowseResponse {
        TagBrowseResponse::Success(self.tag_browse_payload())
    }

    fn tag_browse_payload(&self) -> TagBrowsePayload {
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
        tag_browse_payload_from_summaries(db.tag_summaries(), &self.settings.tags)
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
        let sort_order = self
            .sort_settings
            .load()
            .map_err(|error| internal_error("最新の並び順を読み込めませんでした", error))?;
        let db_path = crate::tags_db::TagsDb::db_path();
        if !db_path.try_exists().unwrap_or(false) {
            return Ok(self.tag_items_state_payload(sort_order, TagIndexState::Unavailable));
        }
        let db = match crate::tags_db::TagsDb::open_readonly(&db_path) {
            Ok(db) => db,
            Err(error) => {
                crate::logger::log(format!("remote_ipc: tags index open failed: {error}"));
                return Ok(self.tag_items_state_payload(sort_order, TagIndexState::Unavailable));
            }
        };
        match db.has_any_tags() {
            Ok(true) => {}
            Ok(false) => {
                return Ok(self.tag_items_state_payload(sort_order, TagIndexState::Empty));
            }
            Err(error) => {
                crate::logger::log(format!("remote_ipc: tags index query failed: {error}"));
                return Ok(self.tag_items_state_payload(sort_order, TagIndexState::Unavailable));
            }
        }
        self.tag_items_from_db(&db, &request, sort_order)
    }

    fn tag_items_from_db(
        &self,
        db: &crate::tags_db::TagsDb,
        request: &TagItemsRequest,
        sort_order: crate::settings::SortOrder,
    ) -> Result<TagItemsPayload, CollectionError> {
        let favorites = self.favorite_roots.current().map_err(|error| {
            crate::logger::log(format!("remote_ipc: {error}"));
            CollectionError::new(
                CollectionErrorCode::Internal,
                "最新のお気に入りを読み込めませんでした",
            )
        })?;
        let key_scan_limit = tag_item_key_scan_limit(request.kind);
        let item_keys =
            crate::tag_view::select_tag_view_item_keys(db, request.tag.trim(), key_scan_limit + 1);
        let (entries, truncated) =
            map_tag_item_keys(&favorites, item_keys, request.kind, key_scan_limit);
        Ok(TagItemsPayload {
            listing: self.tag_items_listing(sort_order, entries, truncated),
            state: TagIndexState::Ready,
        })
    }

    fn tag_items_state_payload(
        &self,
        sort_order: crate::settings::SortOrder,
        state: TagIndexState,
    ) -> TagItemsPayload {
        TagItemsPayload {
            listing: self.tag_items_listing(sort_order, Vec::new(), false),
            state,
        }
    }

    fn tag_items_listing(
        &self,
        sort_order: crate::settings::SortOrder,
        entries: Vec<RemoteEntry>,
        truncated: bool,
    ) -> CollectionPayload {
        CollectionPayload {
            title: "タグの項目".to_owned(),
            thumb_aspect_height_ratio: aggregate_thumb_aspect_height_ratio(&self.settings),
            sort_state: super::remote_grid_sort_state(
                sort_order,
                Some(super::FIXED_LIST_SORT_LOCK_REASON),
            ),
            entries,
            entry_limit: MAX_REMOTE_COLLECTION_ENTRIES,
            truncated,
        }
    }

    fn reading_history(
        &self,
        sort_order: crate::settings::SortOrder,
    ) -> Result<CollectionPayload, CollectionError> {
        let rows = if crate::reading_history_db::ReadingHistoryDb::db_path()
            .try_exists()
            .unwrap_or(false)
        {
            crate::reading_history_db::ReadingHistoryDb::open_readonly()
                .and_then(|db| db.list_recent(self.settings.reading_history_limit))
                .map_err(|error| internal_error("閲覧履歴を読み込めませんでした", error))?
        } else {
            Vec::new()
        };
        let candidates = rows
            .into_iter()
            .map(candidate_from_reading_history_entry)
            .collect();
        self.payload(
            "閲覧履歴",
            candidates,
            sort_order,
            Some(super::FIXED_LIST_SORT_LOCK_REASON),
        )
    }

    fn rating(
        &self,
        stars: u8,
        sort_order: crate::settings::SortOrder,
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
            &format!("レーティング ★{stars}"),
            candidates,
            sort_order,
            Some(super::FIXED_LIST_SORT_LOCK_REASON),
        )
    }

    fn bookmarks(
        &self,
        sort_order: crate::settings::SortOrder,
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
            "ブックマーク",
            candidates,
            sort_order,
            Some(super::FIXED_LIST_SORT_LOCK_REASON),
        )
    }

    fn bookshelf(
        &self,
        sort_order: crate::settings::SortOrder,
    ) -> Result<CollectionPayload, CollectionError> {
        let root = self.settings.books_root_path();
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
            "本棚",
            candidates,
            sort_order,
            Some(super::FIXED_LIST_SORT_LOCK_REASON),
        )
    }

    fn smart_folder(
        &self,
        definition_id: &str,
        sort_order: crate::settings::SortOrder,
    ) -> Result<CollectionPayload, CollectionError> {
        let id = uuid::Uuid::parse_str(definition_id).map_err(|_| {
            CollectionError::new(CollectionErrorCode::BadRequest, "ID が正しくありません")
        })?;
        let definition = self
            .settings
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
        let mut settings = self.settings.clone();
        settings.sort_order = sort_order;
        let entries =
            crate::app::smart_folder::build_remote_smart_folder_entries(&settings, definition)
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
        self.payload(&title, candidates, sort_order, None)
    }

    fn payload(
        &self,
        title: &str,
        candidates: Vec<CandidateEntry>,
        sort_order: crate::settings::SortOrder,
        locked_reason: Option<&str>,
    ) -> Result<CollectionPayload, CollectionError> {
        let favorites = self.favorite_roots.current().map_err(|error| {
            crate::logger::log(format!("remote_ipc: {error}"));
            CollectionError::new(
                CollectionErrorCode::Internal,
                "最新のお気に入りを読み込めませんでした",
            )
        })?;
        let entries = to_remote_entries(&favorites, candidates);
        let (entries, truncated) = bound_remote_entries(entries);
        Ok(CollectionPayload {
            title: title.to_owned(),
            thumb_aspect_height_ratio: aggregate_thumb_aspect_height_ratio(&self.settings),
            sort_state: super::remote_grid_sort_state(sort_order, locked_reason),
            entries,
            entry_limit: MAX_REMOTE_COLLECTION_ENTRIES,
            truncated,
        })
    }
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

fn map_tag_item_keys(
    favorites: &[FavoriteEntry],
    item_keys: Vec<String>,
    filter: TagItemKind,
    key_scan_limit: usize,
) -> (Vec<RemoteEntry>, bool) {
    let key_limit_reached = item_keys.len() > key_scan_limit;
    let roots = resolve_existing_favorite_roots(favorites);
    let mut entries = Vec::with_capacity(MAX_REMOTE_COLLECTION_ENTRIES + 1);
    for key in item_keys.into_iter().take(key_scan_limit) {
        let Some(candidate) = candidate_from_tag_item_key(key, filter) else {
            continue;
        };
        let Some(entry) = remote_entry_from_candidate(&roots, candidate) else {
            continue;
        };
        entries.push(entry);
        if entries.len() > MAX_REMOTE_COLLECTION_ENTRIES {
            break;
        }
    }
    let (entries, matched_truncated) = bound_remote_entries(entries);
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

fn bound_remote_entries(mut entries: Vec<RemoteEntry>) -> (Vec<RemoteEntry>, bool) {
    let truncated = entries.len() > MAX_REMOTE_COLLECTION_ENTRIES;
    entries.truncate(MAX_REMOTE_COLLECTION_ENTRIES);
    (entries, truncated)
}

fn visible_places(settings: &Settings) -> Vec<PlaceSummary> {
    let mut places = Vec::new();
    if settings.show_location_reading_history {
        places.push(PlaceSummary {
            kind: PlaceKind::ReadingHistory,
            name: "閲覧履歴".to_owned(),
        });
    }
    if settings.show_location_rating {
        places.push(PlaceSummary {
            kind: PlaceKind::Rating,
            name: "レーティング".to_owned(),
        });
    }
    if settings.show_location_bookshelf {
        places.push(PlaceSummary {
            kind: PlaceKind::Bookshelf,
            name: "本棚".to_owned(),
        });
    }
    // 本体の「場所▼」にもブックマーク専用の非表示設定はない。
    places.push(PlaceSummary {
        kind: PlaceKind::Bookmarks,
        name: "ブックマーク".to_owned(),
    });
    places
}

fn to_remote_entries(
    favorites: &[FavoriteEntry],
    candidates: Vec<CandidateEntry>,
) -> Vec<RemoteEntry> {
    to_remote_entries_bounded(favorites, candidates, usize::MAX)
}

fn to_remote_entries_bounded(
    favorites: &[FavoriteEntry],
    candidates: Vec<CandidateEntry>,
    mapped_limit: usize,
) -> Vec<RemoteEntry> {
    let roots = resolve_existing_favorite_roots(favorites);
    candidates
        .into_iter()
        .filter_map(|candidate| remote_entry_from_candidate(&roots, candidate))
        .take(mapped_limit)
        .collect()
}

fn remote_entry_from_candidate(
    roots: &[ResolvedFavoriteRoot<'_>],
    candidate: CandidateEntry,
) -> Option<RemoteEntry> {
    let mapped = map_existing_to_resolved_favorite(roots, &candidate.path)?;
    Some(RemoteEntry {
        root_id: mapped.root_id,
        relative_path: mapped.relative_path,
        name: if candidate.name.trim().is_empty() {
            file_name(&candidate.path)
        } else {
            candidate.name
        },
        kind: candidate.kind,
        detail: candidate.detail,
        progress_current: candidate.progress_current,
        progress_total: candidate.progress_total,
        rating: candidate.rating,
    })
}

fn map_favorite_search_entries(
    favorites: &[FavoriteEntry],
    index_entries: Vec<IndexEntry>,
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
    let entries =
        to_remote_entries_bounded(favorites, candidates, MAX_REMOTE_COLLECTION_ENTRIES + 1);
    let (entries, mapped_truncated) = bound_remote_entries(entries);
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
        settings.show_location_reading_history = false;
        settings.show_location_rating = false;
        settings.show_location_bookshelf = true;
        let places = visible_places(&settings);
        assert_eq!(
            places.iter().map(|place| place.kind).collect::<Vec<_>>(),
            [PlaceKind::Bookshelf, PlaceKind::Bookmarks]
        );
    }

    #[test]
    fn favorite_allowlist_drops_outside_entries_and_returns_only_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let inside = root.join("album/page.jpg");
        let outside = temp.path().join("outside.jpg");
        std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
        std::fs::write(&inside, b"inside").unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        let favorite = FavoriteEntry::new("favorite".to_owned(), root);
        let candidates = vec![
            CandidateEntry {
                path: inside,
                name: "page.jpg".to_owned(),
                kind: RemoteEntryKind::Image,
                detail: None,
                progress_current: None,
                progress_total: None,
                rating: None,
            },
            CandidateEntry {
                path: outside,
                name: "outside.jpg".to_owned(),
                kind: RemoteEntryKind::Image,
                detail: None,
                progress_current: None,
                progress_total: None,
                rating: None,
            },
        ];
        let entries = to_remote_entries(std::slice::from_ref(&favorite), candidates);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].root_id, favorite.id.to_string());
        assert_eq!(entries[0].relative_path, "album/page.jpg");
        assert!(!Path::new(&entries[0].relative_path).is_absolute());
        let json = serde_json::to_string(&entries).unwrap();
        assert!(!json.contains(&temp.path().to_string_lossy().to_string()));
    }

    #[test]
    fn aggregate_payload_is_bounded_to_one_thousand_entries() {
        let entries = (0..=MAX_REMOTE_COLLECTION_ENTRIES)
            .map(|index| RemoteEntry {
                root_id: "00000000-0000-0000-0000-000000000000".to_owned(),
                relative_path: format!("entry-{index}"),
                name: format!("entry-{index}"),
                kind: RemoteEntryKind::Image,
                detail: None,
                progress_current: None,
                progress_total: None,
                rating: None,
            })
            .collect();
        let (entries, truncated) = bound_remote_entries(entries);
        assert_eq!(entries.len(), MAX_REMOTE_COLLECTION_ENTRIES);
        assert!(truncated);
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
    fn tag_items_drop_outside_and_missing_paths_without_changing_tags_db() {
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
        assert_eq!(payload.listing.entries.len(), 1);
        assert_eq!(payload.listing.entries[0].relative_path, "inside.jpg");
        let json = serde_json::to_string(&payload).unwrap();
        assert!(!json.contains(&data_dir.path().to_string_lossy().to_string()));
        let db = crate::tags_db::TagsDb::open_readonly(&crate::tags_db::TagsDb::db_path()).unwrap();
        assert_eq!(db.display_tags_for_item(&missing_key), vec!["#cat"]);
        assert!(db.has_item_state(&missing_key));
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
                .find(|entry| entry.relative_path == "notes.unknown-extension")
                .map(|entry| entry.kind),
            Some(RemoteEntryKind::Other)
        );
        let folders = tag_items_success(engine.tag_items(TagItemsRequest {
            tag: "cat".to_owned(),
            kind: TagItemKind::Folder,
        }));
        assert_eq!(folders.listing.entries.len(), 1);
        assert_eq!(folders.listing.entries[0].relative_path, "folder");
        assert_eq!(folders.listing.entries[0].kind, RemoteEntryKind::Folder);
    }

    #[test]
    fn tag_item_mapping_stops_at_one_over_the_response_limit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let image = root.join("image.jpg");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&image, b"image").unwrap();
        let favorite = FavoriteEntry::new("favorite".to_owned(), root);
        let item_key = crate::tags_db::item_key_for_path(&image);
        let item_keys = vec![item_key; MAX_REMOTE_COLLECTION_ENTRIES + 1];

        let (entries, truncated) = map_tag_item_keys(
            std::slice::from_ref(&favorite),
            item_keys,
            TagItemKind::All,
            crate::tag_view::TAG_VIEW_RESULT_LIMIT,
        );

        assert_eq!(entries.len(), MAX_REMOTE_COLLECTION_ENTRIES);
        assert!(truncated);
    }

    #[test]
    fn tag_item_kind_filter_scans_past_the_first_response_window() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        std::fs::create_dir_all(&root).unwrap();
        let mut item_keys = Vec::new();
        for index in 0..=MAX_REMOTE_COLLECTION_ENTRIES {
            let path = root.join(format!("{index:04}.mp4"));
            std::fs::write(&path, b"video").unwrap();
            item_keys.push(crate::tags_db::item_key_for_path(&path));
        }
        let image = root.join("zzzz.jpg");
        std::fs::write(&image, b"image").unwrap();
        item_keys.push(crate::tags_db::item_key_for_path(&image));
        let favorite = FavoriteEntry::new("favorite".to_owned(), root);

        let (entries, truncated) = map_tag_item_keys(
            std::slice::from_ref(&favorite),
            item_keys,
            TagItemKind::Image,
            crate::tag_view::TAG_VIEW_FILTERED_KEY_SCAN_LIMIT,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, "zzzz.jpg");
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
    fn favorite_search_drops_index_paths_outside_the_live_allowlist() {
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
        assert_eq!(payload.listing.entries.len(), 1);
        assert_eq!(payload.listing.entries[0].root_id, favorite.id.to_string());
        assert_eq!(payload.listing.entries[0].relative_path, "match-inside");
        assert!(!Path::new(&payload.listing.entries[0].relative_path).is_absolute());
        let json = serde_json::to_string(&payload).unwrap();
        assert!(!json.contains(&data_dir.path().to_string_lossy().to_string()));
    }

    #[test]
    fn favorite_search_mapping_stops_at_one_over_the_response_limit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let target = root.join("album");
        std::fs::create_dir_all(&target).unwrap();
        let favorite = favorite_with_container_index(root, true);
        let index_entries = (0..=MAX_REMOTE_COLLECTION_ENTRIES)
            .map(|index| IndexEntry {
                path: target.clone(),
                display_name: format!("album-{index}"),
                kind: IndexKind::Folder,
                mtime: 0,
            })
            .collect();

        let (entries, truncated) =
            map_favorite_search_entries(std::slice::from_ref(&favorite), index_entries);

        assert_eq!(entries.len(), MAX_REMOTE_COLLECTION_ENTRIES);
        assert!(truncated);
    }
}
