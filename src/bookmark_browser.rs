//! 動画・音声・本を横断するブックマーク一覧の read model と worker。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

/// 「状態 > ブックマークあり/なし」の在メモリ判定用スナップショット。
///
/// DB 全件読み出しは worker で行い、一覧の再評価中はこの集合だけを参照する。
#[derive(Clone, Debug, Default)]
pub struct BookmarkPresence {
    media_keys: HashSet<String>,
    book_container_keys: HashSet<String>,
    book_page_keys: HashSet<(String, String)>,
    archive_entries_by_container: HashMap<String, Vec<String>>,
}

impl BookmarkPresence {
    pub(crate) fn from_rows(
        media_keys: HashSet<String>,
        books: Vec<crate::book_bookmarks::BookBookmark>,
    ) -> Self {
        let media_keys = media_keys
            .into_iter()
            .map(|key| crate::path_key::normalize_keep_drive(Path::new(&key)))
            .collect();
        let mut presence = Self {
            media_keys,
            ..Self::default()
        };
        for bookmark in books {
            let container_key = crate::book_bookmarks::container_key(&bookmark.container_path);
            let page_key = book_page_identity_key(&bookmark.page_identity);
            presence.book_container_keys.insert(container_key.clone());
            presence
                .book_page_keys
                .insert((container_key.clone(), page_key));
            if let crate::book_bookmarks::PageIdentity::ArchiveEntry(entry) = bookmark.page_identity
            {
                presence
                    .archive_entries_by_container
                    .entry(container_key)
                    .or_default()
                    .push(normalize_virtual_path(&entry));
            }
        }
        presence
    }

    pub fn has_media_path(&self, path: &Path) -> bool {
        self.media_keys
            .contains(&crate::path_key::normalize_keep_drive(path))
    }

    pub fn has_book_container(&self, path: &Path) -> bool {
        self.book_container_keys
            .contains(&crate::book_bookmarks::container_key(path))
    }

    pub fn has_book_page(
        &self,
        container_path: &Path,
        identity: &crate::book_bookmarks::PageIdentity,
    ) -> bool {
        self.book_page_keys.contains(&(
            crate::book_bookmarks::container_key(container_path),
            book_page_identity_key(identity),
        ))
    }

    pub fn has_archive_prefix(&self, container_path: &Path, prefix: &str) -> bool {
        let container_key = crate::book_bookmarks::container_key(container_path);
        let prefix = normalize_virtual_path(prefix);
        self.archive_entries_by_container
            .get(&container_key)
            .is_some_and(|entries| entries.iter().any(|entry| entry.starts_with(&prefix)))
    }
}

fn normalize_virtual_path(value: &str) -> String {
    value.replace('\\', "/").to_lowercase()
}

fn book_page_identity_key(identity: &crate::book_bookmarks::PageIdentity) -> String {
    match identity {
        crate::book_bookmarks::PageIdentity::RelativePath(value) => {
            format!("relative:{}", normalize_virtual_path(value))
        }
        crate::book_bookmarks::PageIdentity::ArchiveEntry(value) => {
            format!("archive:{}", normalize_virtual_path(value))
        }
        crate::book_bookmarks::PageIdentity::PdfPage(page) => format!("pdf:{page}"),
    }
}

pub struct BookmarkPresencePending {
    cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<Result<BookmarkPresence, String>>,
}

impl BookmarkPresencePending {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn spawn_presence_build() -> BookmarkPresencePending {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("bookmark-presence-build".to_string())
        .spawn(move || {
            let result = load_presence().and_then(|presence| {
                if cancel_worker.load(Ordering::Relaxed) {
                    Err("cancelled".to_string())
                } else {
                    Ok(presence)
                }
            });
            let _ = tx.send(result);
        });
    BookmarkPresencePending { cancel, rx }
}

pub(crate) fn load_presence() -> Result<BookmarkPresence, String> {
    let media_keys = crate::video_bookmarks::VideoBookmarkDb::open()
        .and_then(|db| db.list_all_path_keys())
        .map_err(|error| format!("動画・音声ブックマーク DB を読み込めませんでした: {error}"))?;
    let books = crate::book_bookmarks::load_all_from_disk()
        .map_err(|error| format!("本ブックマーク DB を読み込めませんでした: {error}"))?;
    Ok(BookmarkPresence::from_rows(media_keys, books))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MediaFilter {
    #[default]
    All,
    Video,
    Audio,
    Book,
}

impl MediaFilter {
    pub const ALL: [Self; 4] = [Self::All, Self::Video, Self::Audio, Self::Book];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "すべて",
            Self::Video => "動画",
            Self::Audio => "音声",
            Self::Book => "本",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BookKindFilter {
    #[default]
    All,
    ImageFolder,
    Zip,
    Pdf,
    OtherArchive,
}

impl BookKindFilter {
    pub const ALL: [Self; 5] = [
        Self::All,
        Self::ImageFolder,
        Self::Zip,
        Self::Pdf,
        Self::OtherArchive,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "すべて",
            Self::ImageFolder => "画像フォルダ",
            Self::Zip => "ZIP・CBZ",
            Self::Pdf => "PDF",
            Self::OtherArchive => "その他アーカイブ",
        }
    }

    pub fn matches(self, kind: crate::book_bookmarks::BookContainerKind) -> bool {
        use crate::book_bookmarks::BookContainerKind;
        match self {
            Self::All => true,
            Self::ImageFolder => matches!(
                kind,
                BookContainerKind::CompiledBook | BookContainerKind::ImageFolder
            ),
            Self::Zip => kind == BookContainerKind::Zip,
            Self::Pdf => kind == BookContainerKind::Pdf,
            Self::OtherArchive => kind == BookContainerKind::OtherArchive,
        }
    }
}

#[derive(Clone, Debug)]
pub enum BookmarkRowSource {
    Media {
        id: i64,
        path: PathBuf,
        pts_secs: f64,
        title: Option<String>,
        is_audio: bool,
    },
    Book(crate::book_bookmarks::BookBookmark),
}

#[derive(Clone, Debug)]
pub struct BookmarkBrowserRow {
    pub source: BookmarkRowSource,
    pub created_at_ms: i64,
    pub missing: bool,
}

impl BookmarkBrowserRow {
    pub fn media_filter(&self) -> MediaFilter {
        match &self.source {
            BookmarkRowSource::Media { is_audio: true, .. } => MediaFilter::Audio,
            BookmarkRowSource::Media { .. } => MediaFilter::Video,
            BookmarkRowSource::Book(_) => MediaFilter::Book,
        }
    }

    pub fn stable_key(&self) -> (u8, i64) {
        match &self.source {
            BookmarkRowSource::Media { id, .. } => (0, *id),
            BookmarkRowSource::Book(bookmark) => (1, bookmark.id),
        }
    }
}

pub struct BookmarkBrowserPending {
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<Result<Vec<BookmarkBrowserRow>, String>>,
}

impl BookmarkBrowserPending {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn spawn_build() -> BookmarkBrowserPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("bookmark-browser-build".to_string())
        .spawn(move || {
            let result = build_rows(&cancel_worker).map_err(|err| err.to_string());
            let _ = tx.send(result);
        })
        .ok();
    BookmarkBrowserPending { cancel, rx }
}

fn build_rows(cancel: &AtomicBool) -> Result<Vec<BookmarkBrowserRow>, rusqlite::Error> {
    let mut rows = Vec::new();
    let media = crate::video_bookmarks::VideoBookmarkDb::open()?.list_all_global()?;
    for bookmark in media {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Vec::new());
        }
        let ext = bookmark
            .path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_lowercase();
        let is_audio = crate::folder_tree::SUPPORTED_AUDIO_EXTENSIONS.contains(&ext.as_str());
        let missing = !bookmark.path.try_exists().unwrap_or(false);
        rows.push(BookmarkBrowserRow {
            source: BookmarkRowSource::Media {
                id: bookmark.id,
                path: bookmark.path,
                pts_secs: bookmark.pts_secs,
                title: bookmark.title,
                is_audio,
            },
            created_at_ms: bookmark.created_at_ms,
            missing,
        });
    }
    for bookmark in crate::book_bookmarks::load_all_from_disk()? {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Vec::new());
        }
        let missing = book_page_missing(&bookmark);
        rows.push(BookmarkBrowserRow {
            created_at_ms: bookmark.created_at_ms,
            source: BookmarkRowSource::Book(bookmark),
            missing,
        });
    }
    rows.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    Ok(rows)
}

fn book_page_missing(bookmark: &crate::book_bookmarks::BookBookmark) -> bool {
    if !bookmark.container_path.try_exists().unwrap_or(false) {
        return true;
    }
    match (&bookmark.container_kind, &bookmark.page_identity) {
        (
            crate::book_bookmarks::BookContainerKind::CompiledBook
            | crate::book_bookmarks::BookContainerKind::ImageFolder,
            crate::book_bookmarks::PageIdentity::RelativePath(relative),
        ) => !bookmark
            .container_path
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
            .try_exists()
            .unwrap_or(false),
        (
            crate::book_bookmarks::BookContainerKind::Zip,
            crate::book_bookmarks::PageIdentity::ArchiveEntry(wanted),
        ) => crate::zip_loader::enumerate_image_entries(&bookmark.container_path)
            .map(|entries| {
                let wanted = wanted.replace('\\', "/").to_lowercase();
                !entries
                    .into_iter()
                    .any(|entry| entry.entry_name.replace('\\', "/").to_lowercase() == wanted)
            })
            // 読み取りエラーや一時ロックは「missing」と断定しない。
            .unwrap_or(false),
        (
            crate::book_bookmarks::BookContainerKind::Pdf,
            crate::book_bookmarks::PageIdentity::PdfPage(page),
        ) => crate::pdf_loader::enumerate_pages(&bookmark.container_path, None)
            .map(|pages| *page as usize >= pages.len())
            // パスワード付き PDF 等は open 時に既存導線で解決する。
            .unwrap_or(false),
        _ => false,
    }
}

pub struct BookmarkDeletePending {
    pub key: (u8, i64),
    pub rx: mpsc::Receiver<Result<(), String>>,
}

pub fn spawn_delete(row: &BookmarkBrowserRow) -> BookmarkDeletePending {
    let key = row.stable_key();
    let source = row.source.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("bookmark-browser-delete".to_string())
        .spawn(move || {
            let result = match source {
                BookmarkRowSource::Media { id, .. } => {
                    crate::video_bookmarks::VideoBookmarkDb::open()
                        .and_then(|db| db.remove(id))
                        .map_err(|err| err.to_string())
                }
                BookmarkRowSource::Book(bookmark) => {
                    crate::book_bookmarks::remove_from_disk(bookmark.id)
                        .map_err(|err| err.to_string())
                }
            };
            let _ = tx.send(result);
        })
        .ok();
    BookmarkDeletePending { key, rx }
}

#[derive(Clone, Debug)]
pub struct PendingMediaOpen {
    pub path: PathBuf,
    pub pts_secs: f64,
    pub started_at: std::time::Instant,
}

#[derive(Clone, Debug)]
pub struct PendingBookOpen {
    pub bookmark: crate::book_bookmarks::BookBookmark,
    pub started_at: std::time::Instant,
    pub entered_archive_prefix: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn book_kind_filter_groups_compiled_with_image_folders() {
        assert!(
            BookKindFilter::ImageFolder
                .matches(crate::book_bookmarks::BookContainerKind::CompiledBook)
        );
        assert!(
            BookKindFilter::ImageFolder
                .matches(crate::book_bookmarks::BookContainerKind::ImageFolder)
        );
        assert!(
            !BookKindFilter::ImageFolder.matches(crate::book_bookmarks::BookContainerKind::Pdf)
        );
    }

    #[test]
    fn presence_matches_media_containers_pages_and_archive_prefixes() {
        use crate::book_bookmarks::{BookBookmark, BookContainerKind, PageIdentity};

        let media = HashSet::from([r"C:\Media\Clip.MP4".to_string()]);
        let folder = PathBuf::from(r"C:\Books\FolderBook");
        let archive = PathBuf::from(r"C:\Books\Story.CBZ");
        let rows = vec![
            BookBookmark {
                id: 1,
                container_key: crate::book_bookmarks::container_key(&folder),
                container_path: folder.clone(),
                container_kind: BookContainerKind::ImageFolder,
                page_identity: PageIdentity::RelativePath("Chapter/001.JPG".into()),
                page_index_hint: 0,
                created_at_ms: 1,
            },
            BookBookmark {
                id: 2,
                container_key: crate::book_bookmarks::container_key(&archive),
                container_path: archive.clone(),
                container_kind: BookContainerKind::Zip,
                page_identity: PageIdentity::ArchiveEntry("Part/002.PNG".into()),
                page_index_hint: 1,
                created_at_ms: 2,
            },
        ];
        let presence = BookmarkPresence::from_rows(media, rows);

        assert!(presence.has_media_path(Path::new(r"c:\media\clip.mp4")));
        assert!(presence.has_book_container(&folder));
        assert!(presence.has_book_page(
            &folder,
            &PageIdentity::RelativePath("chapter\\001.jpg".into())
        ));
        assert!(presence.has_archive_prefix(&archive, "part/"));
        assert!(!presence.has_archive_prefix(&archive, "other/"));
    }
}
