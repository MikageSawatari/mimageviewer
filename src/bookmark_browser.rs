//! 動画・音声・本を横断するブックマーク一覧の read model と worker。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

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
}
