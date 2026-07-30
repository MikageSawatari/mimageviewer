use std::path::{Path, PathBuf};

use mimageviewer_ipc::{
    CollectionError, CollectionErrorCode, CollectionKind, CollectionPayload, CollectionRequest,
    CollectionResponse, HomePayload, HomeResponse, PlaceKind, PlaceSummary, RemoteEntry,
    RemoteEntryKind, SmartFolderSummary,
};

use crate::grid_item::GridItem;
use crate::settings::{FavoriteEntry, Settings};

use super::path_guard::map_existing_to_favorite;

pub(super) struct CollectionEngine {
    settings: Settings,
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
        Self { settings }
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
        let result = match request.kind {
            CollectionKind::ReadingHistory => self.reading_history(),
            CollectionKind::Rating { stars } => self.rating(stars),
            CollectionKind::Bookshelf => self.bookshelf(),
            CollectionKind::Bookmarks => self.bookmarks(),
            CollectionKind::SmartFolder { definition_id } => self.smart_folder(&definition_id),
        };
        match result {
            Ok(payload) => CollectionResponse::Success(payload),
            Err(error) => CollectionResponse::Error(error),
        }
    }

    fn reading_history(&self) -> Result<CollectionPayload, CollectionError> {
        let rows = if crate::reading_history_db::ReadingHistoryDb::db_path()
            .try_exists()
            .unwrap_or(false)
        {
            crate::reading_history_db::ReadingHistoryDb::open_readonly()
                .and_then(|db| db.list_recent(self.settings.reading_history_limit))
                .map_err(|error| internal_error("読書履歴を読み込めませんでした", error))?
        } else {
            Vec::new()
        };
        let candidates = rows
            .into_iter()
            .map(|entry| {
                let current = entry.last_page.map(|page| page.max(0) as u64 + 1);
                let total = entry.page_count.map(|count| count.max(0) as u64);
                CandidateEntry {
                    path: entry.path,
                    name: entry.title,
                    kind: match entry.kind {
                        crate::reading_history_db::ReadingHistoryKind::Folder => {
                            RemoteEntryKind::Folder
                        }
                        crate::reading_history_db::ReadingHistoryKind::Zip => RemoteEntryKind::Zip,
                        crate::reading_history_db::ReadingHistoryKind::Pdf => RemoteEntryKind::Pdf,
                        crate::reading_history_db::ReadingHistoryKind::Archive => {
                            RemoteEntryKind::Archive
                        }
                    },
                    detail: progress_label(current, total),
                    progress_current: current,
                    progress_total: total,
                    rating: None,
                }
            })
            .collect();
        Ok(self.payload("読書履歴", candidates))
    }

    fn rating(&self, stars: u8) -> Result<CollectionPayload, CollectionError> {
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
        Ok(self.payload(&format!("レーティング ★{stars}"), candidates))
    }

    fn bookmarks(&self) -> Result<CollectionPayload, CollectionError> {
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
        Ok(self.payload("ブックマーク", candidates))
    }

    fn bookshelf(&self) -> Result<CollectionPayload, CollectionError> {
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
        Ok(self.payload("本棚", candidates))
    }

    fn smart_folder(&self, definition_id: &str) -> Result<CollectionPayload, CollectionError> {
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
        let entries =
            crate::app::smart_folder::build_remote_smart_folder_entries(&self.settings, definition)
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
        Ok(self.payload(&title, candidates))
    }

    fn payload(&self, title: &str, candidates: Vec<CandidateEntry>) -> CollectionPayload {
        CollectionPayload {
            title: title.to_owned(),
            thumb_aspect_height_ratio: aggregate_thumb_aspect_height_ratio(&self.settings),
            entries: to_remote_entries(&self.settings.favorites, candidates),
        }
    }
}

fn visible_places(settings: &Settings) -> Vec<PlaceSummary> {
    let mut places = Vec::new();
    if settings.show_location_reading_history {
        places.push(PlaceSummary {
            kind: PlaceKind::ReadingHistory,
            name: "読書履歴".to_owned(),
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
    candidates
        .into_iter()
        .filter_map(|candidate| {
            let mapped = map_existing_to_favorite(favorites, &candidate.path)?;
            Some(RemoteEntry {
                favorite_id: mapped.favorite_id,
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
        })
        .collect()
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

fn aggregate_thumb_aspect_height_ratio(settings: &Settings) -> f64 {
    if settings.thumb_aspect_auto {
        1.0
    } else {
        f64::from(settings.thumb_aspect.height_ratio())
    }
}

fn progress_label(current: Option<u64>, total: Option<u64>) -> Option<String> {
    match (current, total) {
        (Some(current), Some(total)) if total > 0 => Some(format!("{current} / {total} ページ")),
        (Some(current), _) => Some(format!("{current} ページ")),
        _ => None,
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
        assert_eq!(entries[0].favorite_id, favorite.id.to_string());
        assert_eq!(entries[0].relative_path, "album/page.jpg");
        assert!(!Path::new(&entries[0].relative_path).is_absolute());
        let json = serde_json::to_string(&entries).unwrap();
        assert!(!json.contains(&temp.path().to_string_lossy().to_string()));
    }
}
