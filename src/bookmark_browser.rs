//! 動画・音声・本を横断するブックマーク一覧の read model と worker。
//!
//! 一覧は専用ダイアログではなく、通常の `App.items` を使う最上位グリッドへ install する。
//! このモジュールの row は `GridItem` に載らないブックマーク ID / 再生位置 / 登録日時と、
//! 動画 marker の保存済みサムネイルを保持する sidecar である。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::grid_item::GridItem;

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BookmarkViewSort {
    Normal(crate::settings::SortOrder),
    CreatedAtDesc,
    CreatedAtAsc,
}

impl Default for BookmarkViewSort {
    fn default() -> Self {
        Self::CreatedAtDesc
    }
}

impl BookmarkViewSort {
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Normal(order) => order.short_label(),
            Self::CreatedAtDesc => "登録日時↓",
            Self::CreatedAtAsc => "登録日時↑",
        }
    }
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

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone)]
pub struct BookmarkBrowserRow {
    pub source: BookmarkRowSource,
    pub item: GridItem,
    /// manifest の relative page を通常 path と区別したまま loader まで運ぶ trust root。
    pub relative_page_provenance: Option<crate::book_bookmarks::RelativePageProvenance>,
    pub image_meta: Option<(i64, i64)>,
    /// 動画・音声 bookmark に保存された位置サムネイル。decode は build worker 側。
    pub marker_thumbnail: Option<Arc<egui::ColorImage>>,
    pub created_at_ms: i64,
    pub missing: bool,
}

impl BookmarkBrowserRow {
    /// 一覧セルの再構築が必要になる表示内容が同じかを判定する。
    /// marker thumbnail は source の保存位置から生成されるため、ここでは有無だけを見る。
    /// 同じ bookmark id をその場で更新できる項目（名称・位置・missing・meta）は個別比較する。
    pub fn has_same_grid_content(&self, other: &Self) -> bool {
        self.source == other.source
            && self.relative_page_provenance == other.relative_page_provenance
            && self.image_meta == other.image_meta
            && self.marker_thumbnail.is_some() == other.marker_thumbnail.is_some()
            && self.created_at_ms == other.created_at_ms
            && self.missing == other.missing
    }

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

    pub fn display_name(&self) -> String {
        match &self.source {
            BookmarkRowSource::Media {
                path,
                pts_secs,
                title,
                ..
            } => {
                let fallback = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                format!(
                    "{} — {}",
                    title.as_deref().unwrap_or(&fallback),
                    format_media_position(*pts_secs)
                )
            }
            BookmarkRowSource::Book(bookmark) => {
                let container = bookmark
                    .container_path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| bookmark.container_path.display().to_string());
                format!(
                    "{} — {}",
                    bookmark.title.as_deref().unwrap_or(&container),
                    bookmark.page_identity.display_name()
                )
            }
        }
    }

    /// 詳細ビューの名前列。位置は専用列に表示されるため、ここでは元コンテナ名を
    /// 常に残し、任意のブックマーク名があれば併記する。
    pub fn details_name(&self) -> String {
        let source_name = self
            .source_path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.source_path().display().to_string());
        match self.title() {
            Some(title) if title != source_name => format!("{source_name} — {title}"),
            _ => source_name,
        }
    }

    /// サムネイル中央に重ねる、ユーザーが明示的に付けた名称。
    /// 未設定時はファイル名を中央表示せず、通常の名前表示へ任せる。
    pub fn title(&self) -> Option<&str> {
        match &self.source {
            BookmarkRowSource::Media { title, .. } => title.as_deref(),
            BookmarkRowSource::Book(bookmark) => bookmark.title.as_deref(),
        }
        .map(str::trim)
        .filter(|title| !title.is_empty())
    }

    pub fn position_label(&self) -> String {
        let base = match &self.source {
            BookmarkRowSource::Media { pts_secs, .. } => format_media_position(*pts_secs),
            BookmarkRowSource::Book(bookmark) => format!(
                "{} / {} ページ",
                bookmark.container_kind.label(),
                bookmark.page_index_hint.saturating_add(1)
            ),
        };
        if self.missing {
            format!("{base} / 見つかりません")
        } else {
            base
        }
    }

    pub fn badge_label(&self) -> String {
        match &self.source {
            BookmarkRowSource::Media { pts_secs, .. } => format_media_position(*pts_secs),
            BookmarkRowSource::Book(bookmark) => {
                format!("P.{}", bookmark.page_index_hint.saturating_add(1))
            }
        }
    }

    pub fn source_path(&self) -> &Path {
        match &self.source {
            BookmarkRowSource::Media { path, .. } => path,
            BookmarkRowSource::Book(bookmark) => &bookmark.container_path,
        }
    }
}

/// worker の返した read model が現在表示中の一覧と同一なら、`start_loading_items` を
/// 再実行する必要はない。表示順はユーザー設定で並べ替え済みなので stable key で照合する。
pub fn rows_have_same_grid_content(
    current: &[BookmarkBrowserRow],
    incoming: &[BookmarkBrowserRow],
) -> bool {
    if current.len() != incoming.len() {
        return false;
    }
    let current_by_key: HashMap<_, _> = current.iter().map(|row| (row.stable_key(), row)).collect();
    incoming.iter().all(|row| {
        current_by_key
            .get(&row.stable_key())
            .is_some_and(|current| current.has_same_grid_content(row))
    })
}

fn format_media_position(pts_secs: f64) -> String {
    let total = pts_secs.max(0.0).round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

pub fn sort_rows(rows: &mut [BookmarkBrowserRow], sort: BookmarkViewSort) {
    match sort {
        BookmarkViewSort::CreatedAtDesc => rows.sort_by(|a, b| {
            b.created_at_ms
                .cmp(&a.created_at_ms)
                .then_with(|| a.stable_key().cmp(&b.stable_key()))
        }),
        BookmarkViewSort::CreatedAtAsc => rows.sort_by(|a, b| {
            a.created_at_ms
                .cmp(&b.created_at_ms)
                .then_with(|| a.stable_key().cmp(&b.stable_key()))
        }),
        BookmarkViewSort::Normal(order) => rows.sort_by(|a, b| {
            let name_a = a.display_name();
            let name_b = b.display_name();
            let key_a = order.name_key(&name_a);
            let key_b = order.name_key(&name_b);
            let mtime_a = a.image_meta.map(|(mtime, _)| mtime).unwrap_or(0);
            let mtime_b = b.image_meta.map(|(mtime, _)| mtime).unwrap_or(0);
            order
                .compare_name_keys(&key_a, mtime_a, &key_b, mtime_b)
                .then_with(|| a.stable_key().cmp(&b.stable_key()))
        }),
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
        let item = if is_audio {
            GridItem::Audio(bookmark.path.clone())
        } else {
            GridItem::Video(bookmark.path.clone())
        };
        let image_meta = source_meta(&bookmark.path)
            .map(|(_, size)| (bookmark.created_at_ms.div_euclid(1000), size));
        let marker_thumbnail = decode_marker_thumbnail(&bookmark.thumb_webp).map(Arc::new);
        rows.push(BookmarkBrowserRow {
            source: BookmarkRowSource::Media {
                id: bookmark.id,
                path: bookmark.path,
                pts_secs: bookmark.pts_secs,
                title: bookmark.title,
                is_audio,
            },
            item,
            relative_page_provenance: None,
            image_meta,
            marker_thumbnail,
            created_at_ms: bookmark.created_at_ms,
            missing,
        });
    }
    let archive_cache = crate::archive_cache::ArchiveCacheDb::open().ok();
    let mut book_missing_cache = HashMap::new();
    for bookmark in crate::book_bookmarks::load_all_from_disk()? {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Vec::new());
        }
        let (mut item, relative_page_missing, mut relative_page_provenance) =
            book_grid_item(&bookmark, archive_cache.as_ref());
        // Filesystem relative page は materialize と存在判定を1回の containment check で
        // 行う。unsafe/missing path を GridItem::Image にせず、後段の metadata / thumb
        // I/O に渡さない。
        let mut missing = relative_page_missing
            .unwrap_or_else(|| book_page_missing(&bookmark, &mut book_missing_cache));
        let mut image_meta = match (&item, relative_page_provenance.as_ref()) {
            (GridItem::Image(_), Some(provenance)) => source_meta_verified(provenance),
            (GridItem::Image(path), None) => source_meta(path),
            _ => source_meta(&bookmark.container_path),
        }
        .map(|(_, size)| (bookmark.created_at_ms.div_euclid(1000), size));
        if relative_page_provenance.is_some() && image_meta.is_none() {
            // materialize 後から source_meta の同一ハンドル検証までに差し替わった場合も、
            // 通常画像として一覧へ流さず missing/invalid 行へ戻す。
            item = GridItem::Folder(bookmark.container_path.clone());
            relative_page_provenance = None;
            missing = true;
            image_meta = None;
        }
        rows.push(BookmarkBrowserRow {
            created_at_ms: bookmark.created_at_ms,
            item,
            relative_page_provenance,
            image_meta,
            marker_thumbnail: None,
            source: BookmarkRowSource::Book(bookmark),
            missing,
        });
    }
    sort_rows(&mut rows, BookmarkViewSort::default());
    Ok(rows)
}

fn source_meta(path: &Path) -> Option<(i64, i64)> {
    let metadata = std::fs::metadata(path).ok()?;
    source_meta_from_metadata(&metadata)
}

fn source_meta_verified(
    provenance: &crate::book_bookmarks::RelativePageProvenance,
) -> Option<(i64, i64)> {
    let opened = provenance.open_verified().ok()?;
    let metadata = opened.metadata().ok()?;
    source_meta_from_metadata(&metadata)
}

fn source_meta_from_metadata(metadata: &std::fs::Metadata) -> Option<(i64, i64)> {
    let mtime = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs()
        .min(i64::MAX as u64) as i64;
    Some((mtime, metadata.len().min(i64::MAX as u64) as i64))
}

fn decode_marker_thumbnail(webp: &[u8]) -> Option<egui::ColorImage> {
    if webp.is_empty() {
        return None;
    }
    let (width, height, rgba) = crate::catalog::decode_thumb_to_rgba(webp)?;
    if width == 0 || height == 0 {
        return None;
    }
    let expected = width as usize * height as usize * 4;
    if rgba.len() != expected {
        return None;
    }
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [width as usize, height as usize],
        &rgba,
    ))
}

fn book_grid_item(
    bookmark: &crate::book_bookmarks::BookBookmark,
    archive_cache: Option<&crate::archive_cache::ArchiveCacheDb>,
) -> (
    GridItem,
    Option<bool>,
    Option<crate::book_bookmarks::RelativePageProvenance>,
) {
    use crate::book_bookmarks::{BookContainerKind, PageIdentity, RelativePagePathResolution};
    match (&bookmark.container_kind, &bookmark.page_identity) {
        (
            BookContainerKind::CompiledBook | BookContainerKind::ImageFolder,
            PageIdentity::RelativePath(relative),
        ) => match crate::book_bookmarks::resolve_relative_page_path(
            &bookmark.container_path,
            relative,
        ) {
            RelativePagePathResolution::Existing(provenance) => (
                GridItem::Image(provenance.candidate_path()),
                Some(false),
                Some(provenance),
            ),
            RelativePagePathResolution::Missing(_) | RelativePagePathResolution::Unsafe => {
                // 行と削除導線は残すが、信頼できない page path は画像として downstream
                // loader へ渡さない。
                (
                    GridItem::Folder(bookmark.container_path.clone()),
                    Some(true),
                    None,
                )
            }
        },
        (BookContainerKind::Pdf, PageIdentity::PdfPage(page_num)) => (
            GridItem::PdfPage {
                pdf_path: bookmark.container_path.clone(),
                page_num: *page_num,
                content_type: None,
            },
            None,
            None,
        ),
        (BookContainerKind::Zip, PageIdentity::ArchiveEntry(entry_name)) => (
            GridItem::ZipImage {
                zip_path: bookmark.container_path.clone(),
                entry_name: entry_name.clone(),
            },
            None,
            None,
        ),
        (BookContainerKind::OtherArchive, PageIdentity::ArchiveEntry(entry_name)) => {
            let backing_path = source_meta(&bookmark.container_path)
                .and_then(|(mtime, size)| {
                    archive_cache.and_then(|db| db.peek(&bookmark.container_path, mtime, size))
                })
                .unwrap_or_else(|| bookmark.container_path.clone());
            (
                GridItem::ZipImage {
                    zip_path: backing_path,
                    entry_name: entry_name.clone(),
                },
                None,
                None,
            )
        }
        // DB 行が将来の kind / identity 組み合わせを持っていても、元コンテナを表示して
        // ブックマーク自体は削除できるよう非破壊側へ倒す。
        _ => (
            match bookmark.container_kind {
                BookContainerKind::CompiledBook | BookContainerKind::ImageFolder => {
                    GridItem::Folder(bookmark.container_path.clone())
                }
                BookContainerKind::Zip => GridItem::ZipFile(bookmark.container_path.clone()),
                BookContainerKind::Pdf => GridItem::PdfFile(bookmark.container_path.clone()),
                BookContainerKind::OtherArchive => GridItem::ConvertibleArchive {
                    path: bookmark.container_path.clone(),
                    format: bookmark
                        .container_path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .and_then(crate::archive_converter::ArchiveFormat::from_extension)
                        .unwrap_or(crate::archive_converter::ArchiveFormat::Zip),
                },
            },
            None,
            None,
        ),
    }
}

enum BookContainerMissingIndex {
    Missing,
    Filesystem,
    ZipEntries(Option<HashSet<String>>),
    PdfPageCount(Option<usize>),
    Present,
}

/// 一覧 worker の1回の構築中は ZIP / PDF の列挙結果をコンテナ単位で共有する。
/// 読み取りエラーは従来どおり missing と断定せず、記録を残して open 時の導線へ委ねる。
fn book_page_missing(
    bookmark: &crate::book_bookmarks::BookBookmark,
    cache: &mut HashMap<String, BookContainerMissingIndex>,
) -> bool {
    let cache_key = crate::book_bookmarks::container_key(&bookmark.container_path);
    let index = cache.entry(cache_key).or_insert_with(|| {
        if !bookmark.container_path.try_exists().unwrap_or(false) {
            return BookContainerMissingIndex::Missing;
        }
        match bookmark.container_kind {
            crate::book_bookmarks::BookContainerKind::CompiledBook
            | crate::book_bookmarks::BookContainerKind::ImageFolder => {
                BookContainerMissingIndex::Filesystem
            }
            crate::book_bookmarks::BookContainerKind::Zip => {
                let entries = crate::zip_loader::enumerate_image_entries(&bookmark.container_path)
                    .ok()
                    .map(|entries| {
                        entries
                            .into_iter()
                            .map(|entry| normalize_virtual_path(&entry.entry_name))
                            .collect()
                    });
                BookContainerMissingIndex::ZipEntries(entries)
            }
            crate::book_bookmarks::BookContainerKind::Pdf => {
                let count = crate::pdf_loader::enumerate_pages(&bookmark.container_path, None)
                    .ok()
                    .map(|pages| pages.len());
                BookContainerMissingIndex::PdfPageCount(count)
            }
            crate::book_bookmarks::BookContainerKind::OtherArchive => {
                BookContainerMissingIndex::Present
            }
        }
    });

    match (index, &bookmark.page_identity) {
        (BookContainerMissingIndex::Missing, _) => true,
        (
            BookContainerMissingIndex::Filesystem,
            crate::book_bookmarks::PageIdentity::RelativePath(relative),
        ) => !matches!(
            crate::book_bookmarks::resolve_relative_page_path(&bookmark.container_path, relative),
            crate::book_bookmarks::RelativePagePathResolution::Existing(_)
        ),
        (
            BookContainerMissingIndex::ZipEntries(entries),
            crate::book_bookmarks::PageIdentity::ArchiveEntry(wanted),
        ) => entries
            .as_ref()
            .map(|entries| !entries.contains(&normalize_virtual_path(wanted)))
            .unwrap_or(false),
        (
            BookContainerMissingIndex::PdfPageCount(count),
            crate::book_bookmarks::PageIdentity::PdfPage(page),
        ) => count.map(|count| *page as usize >= count).unwrap_or(false),
        _ => false,
    }
}

pub struct BookmarkDeletePending {
    pub keys: Vec<(u8, i64)>,
    pub rx: mpsc::Receiver<Result<(), String>>,
}

pub fn spawn_delete(rows: &[BookmarkBrowserRow]) -> BookmarkDeletePending {
    let keys = rows.iter().map(BookmarkBrowserRow::stable_key).collect();
    let sources: Vec<_> = rows.iter().map(|row| row.source.clone()).collect();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("bookmark-browser-delete".to_string())
        .spawn(move || {
            let result = (|| {
                let media_db = sources
                    .iter()
                    .any(|source| matches!(source, BookmarkRowSource::Media { .. }))
                    .then(crate::video_bookmarks::VideoBookmarkDb::open)
                    .transpose()
                    .map_err(|err| err.to_string())?;
                for source in sources {
                    match source {
                        BookmarkRowSource::Media { id, .. } => media_db
                            .as_ref()
                            .expect("media DB opened for media bookmark")
                            .remove(id)
                            .map_err(|err| err.to_string())?,
                        BookmarkRowSource::Book(bookmark) => {
                            crate::book_bookmarks::remove_from_disk(bookmark.id)
                                .map_err(|err| err.to_string())?;
                        }
                    }
                }
                Ok(())
            })();
            let _ = tx.send(result);
        })
        .ok();
    BookmarkDeletePending { keys, rx }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingMediaOpenWait {
    Fullscreen,
    MatchingPath,
    Player,
    PlayerInfo,
}

impl PendingMediaOpenWait {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fullscreen => "fullscreen",
            Self::MatchingPath => "matching_path",
            Self::Player => "player",
            Self::PlayerInfo => "player_info",
        }
    }
}

/// 横断ブックマーク一覧から発行された open request の process-local identity。
///
/// path resolver、viewer 待機、戻り先を同じ要求として照合するために使う。path は同じ対象を
/// 続けて開けるため identity にはならず、単調増加 ID を別に持つ。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BookmarkOpenRequestId(pub(crate) u64);

/// A bookmark open request's stable owner, shared by every asynchronous stage from path
/// resolution through archive conversion and viewer initialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookmarkOpenRequestOwner {
    pub request_id: BookmarkOpenRequestId,
    pub target: BookmarkViewReturnTarget,
}

#[derive(Clone, Debug)]
pub struct PendingMediaOpen {
    pub request_id: BookmarkOpenRequestId,
    pub path: PathBuf,
    pub pts_secs: f64,
    pub started_at: std::time::Instant,
    pub last_wait: Option<PendingMediaOpenWait>,
}

#[derive(Clone, Debug)]
pub enum PendingBookOpenStage {
    /// Path resolution or archive conversion has not yet selected the viewer
    /// context that will own page enumeration.
    Resolving,
    /// The target container is mounted and its page list may still be loading.
    AwaitingPage {
        started_at: std::time::Instant,
        entered_archive_prefix: bool,
    },
}

#[derive(Clone, Debug)]
pub struct PendingBookOpen {
    pub request_id: BookmarkOpenRequestId,
    pub bookmark: crate::book_bookmarks::BookBookmark,
    pub relative_page_provenance: Option<crate::book_bookmarks::RelativePageProvenance>,
    pub started_at: std::time::Instant,
    pub stage: PendingBookOpenStage,
}

impl PendingBookOpen {
    pub fn begin_page_wait(&mut self) {
        self.stage = PendingBookOpenStage::AwaitingPage {
            started_at: std::time::Instant::now(),
            entered_archive_prefix: false,
        };
    }
}

/// A bookmark grid can have only one open request. Keeping media and book
/// requests in one enum prevents a cancelled path resolver from leaving the
/// other request type alive and acting on the next open.
#[derive(Clone, Debug)]
pub enum PendingBookmarkOpen {
    Media(PendingMediaOpen),
    Book(PendingBookOpen),
}

impl PendingBookmarkOpen {
    pub fn request_id(&self) -> BookmarkOpenRequestId {
        match self {
            Self::Media(pending) => pending.request_id,
            Self::Book(pending) => pending.request_id,
        }
    }

    pub fn media(&self) -> Option<&PendingMediaOpen> {
        match self {
            Self::Media(pending) => Some(pending),
            Self::Book(_) => None,
        }
    }

    pub fn media_mut(&mut self) -> Option<&mut PendingMediaOpen> {
        match self {
            Self::Media(pending) => Some(pending),
            Self::Book(_) => None,
        }
    }

    pub fn book(&self) -> Option<&PendingBookOpen> {
        match self {
            Self::Book(pending) => Some(pending),
            Self::Media(_) => None,
        }
    }

    pub fn book_mut(&mut self) -> Option<&mut PendingBookOpen> {
        match self {
            Self::Book(pending) => Some(pending),
            Self::Media(_) => None,
        }
    }
}

/// 横断ブックマーク一覧から開いた viewer が、一覧へ戻れる元の対象。
///
/// メディアは親フォルダではなくファイル自身を保持し、同じフォルダ内の別ファイルへ
/// 移動した場合も元のブックマーク対象から離れたことを判定できるようにする。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BookmarkViewReturnTarget {
    Media(PathBuf),
    Book(PathBuf),
}

impl BookmarkViewReturnTarget {
    pub fn matches_loaded_container(&self, path: &Path) -> bool {
        let container = match self {
            Self::Media(media_path) => media_path.parent().unwrap_or(media_path.as_path()),
            Self::Book(container_path) => container_path.as_path(),
        };
        crate::path_key::eq_keep_drive(container, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[cfg(windows)]
    fn create_dir_link(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }

    #[cfg(unix)]
    fn create_dir_link(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    fn media_row(id: i64, created_at_ms: i64, name: &str) -> BookmarkBrowserRow {
        let path = PathBuf::from(format!(r"C:\media\{name}.mp4"));
        BookmarkBrowserRow {
            source: BookmarkRowSource::Media {
                id,
                path: path.clone(),
                pts_secs: id as f64,
                title: None,
                is_audio: false,
            },
            item: GridItem::Video(path),
            relative_page_provenance: None,
            image_meta: Some((created_at_ms.div_euclid(1000), 10)),
            marker_thumbnail: None,
            created_at_ms,
            missing: false,
        }
    }

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
                title: None,
            },
            BookBookmark {
                id: 2,
                container_key: crate::book_bookmarks::container_key(&archive),
                container_path: archive.clone(),
                container_kind: BookContainerKind::Zip,
                page_identity: PageIdentity::ArchiveEntry("Part/002.PNG".into()),
                page_index_hint: 1,
                created_at_ms: 2,
                title: None,
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

    #[test]
    fn missing_check_reuses_zip_inventory_for_bookmarks_in_same_container() {
        use crate::book_bookmarks::{BookBookmark, BookContainerKind, PageIdentity};

        let temp = tempfile::tempdir().expect("temp dir");
        let zip_path = temp.path().join("book.zip");
        let file = std::fs::File::create(&zip_path).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("chapter/p001.jpg", options)
            .expect("start entry");
        writer.write_all(b"image").expect("write entry");
        writer.finish().expect("finish zip");

        let bookmark = |entry: &str| BookBookmark {
            id: 1,
            container_key: crate::book_bookmarks::container_key(&zip_path),
            container_path: zip_path.clone(),
            container_kind: BookContainerKind::Zip,
            page_identity: PageIdentity::ArchiveEntry(entry.to_string()),
            page_index_hint: 0,
            created_at_ms: 1,
            title: None,
        };
        let mut cache = HashMap::new();
        assert!(!book_page_missing(
            &bookmark("chapter/p001.jpg"),
            &mut cache
        ));

        // 2件目で再列挙すれば読み取りエラーになり missing を断定できない。最初の
        // inventory が再利用されるため、存在しない entry を正しく missing と判定できる。
        std::fs::write(&zip_path, b"not a zip anymore").expect("replace zip");
        assert!(book_page_missing(
            &bookmark("chapter/missing.jpg"),
            &mut cache
        ));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn relative_page_materialization_rechecks_containment_after_path_swap() {
        use crate::book_bookmarks::{BookBookmark, BookContainerKind, PageIdentity};

        let temp = tempfile::tempdir().unwrap();
        let album = temp.path().join("album");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("future.jpg"), b"outside").unwrap();
        let bookmark = BookBookmark {
            id: 1,
            container_key: crate::book_bookmarks::container_key(&album),
            container_path: album.clone(),
            container_kind: BookContainerKind::ImageFolder,
            page_identity: PageIdentity::RelativePath("link/future.jpg".to_string()),
            page_index_hint: 0,
            created_at_ms: 1,
            title: None,
        };

        // import 時点相当: 通常の欠落 path は保持するが画像 item にはしない。
        let (item, missing, provenance) = book_grid_item(&bookmark, None);
        assert!(matches!(item, GridItem::Folder(ref path) if path == &album));
        assert_eq!(missing, Some(true));
        assert!(provenance.is_none());

        // 利用前に欠落 ancestor が外部 link へ置き換わっても、一覧 materialize は
        // external path を GridItem::Image として downstream I/O へ渡さない。
        if !create_dir_link(&outside, &album.join("link")) {
            return;
        }
        let (item, missing, provenance) = book_grid_item(&bookmark, None);
        assert!(matches!(item, GridItem::Folder(ref path) if path == &album));
        assert_eq!(missing, Some(true));
        assert!(provenance.is_none());
    }

    #[test]
    fn existing_safe_relative_page_materializes_as_image() {
        use crate::book_bookmarks::{BookBookmark, BookContainerKind, PageIdentity};

        let temp = tempfile::tempdir().unwrap();
        let album = temp.path().join("album");
        let page = album.join("chapter/page.jpg");
        std::fs::create_dir_all(page.parent().unwrap()).unwrap();
        std::fs::write(&page, b"page").unwrap();
        let bookmark = BookBookmark {
            id: 1,
            container_key: crate::book_bookmarks::container_key(&album),
            container_path: album,
            container_kind: BookContainerKind::ImageFolder,
            page_identity: PageIdentity::RelativePath("chapter/page.jpg".to_string()),
            page_index_hint: 0,
            created_at_ms: 1,
            title: None,
        };

        let (item, missing, provenance) = book_grid_item(&bookmark, None);
        assert!(matches!(item, GridItem::Image(ref path) if path == &page));
        assert_eq!(missing, Some(false));
        assert!(provenance.is_some());
    }

    #[test]
    fn relative_page_source_meta_rejects_swap_after_materialization() {
        let temp = tempfile::tempdir().unwrap();
        let album = temp.path().join("album");
        let chapter = album.join("chapter");
        let parked = album.join("chapter-safe");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&chapter).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(chapter.join("page.jpg"), b"inside").unwrap();
        std::fs::write(outside.join("page.jpg"), vec![0u8; 4096]).unwrap();

        let crate::book_bookmarks::RelativePagePathResolution::Existing(provenance) =
            crate::book_bookmarks::resolve_relative_page_path(&album, "chapter/page.jpg")
        else {
            panic!("safe page should materialize");
        };
        assert!(source_meta_verified(&provenance).is_some());

        std::fs::rename(&chapter, &parked).unwrap();
        if !create_dir_link(&outside, &chapter) {
            return;
        }
        assert!(
            source_meta_verified(&provenance).is_none(),
            "metadata must not come from the swapped external file"
        );
    }

    #[test]
    fn bookmark_view_defaults_to_newest_registration_and_keeps_duplicate_media_rows() {
        let mut rows = vec![
            media_row(1, 1_000, "same"),
            media_row(2, 3_000, "same"),
            media_row(3, 2_000, "other"),
        ];
        sort_rows(&mut rows, BookmarkViewSort::default());
        assert_eq!(
            rows.iter()
                .map(BookmarkBrowserRow::stable_key)
                .collect::<Vec<_>>(),
            vec![(0, 2), (0, 3), (0, 1)]
        );
    }

    #[test]
    fn missing_bookmark_keeps_position_in_display_text() {
        let mut row = media_row(7, 1_000, "clip");
        row.missing = true;
        assert_eq!(row.badge_label(), "0:07");
        assert_eq!(row.position_label(), "0:07 / 見つかりません");
    }

    #[test]
    fn explicit_bookmark_title_is_used_for_name_and_thumbnail_overlay() {
        let path = PathBuf::from(r"C:\Books\story.pdf");
        let row = BookmarkBrowserRow {
            source: BookmarkRowSource::Book(crate::book_bookmarks::BookBookmark {
                id: 9,
                container_key: crate::book_bookmarks::container_key(&path),
                container_path: path.clone(),
                container_kind: crate::book_bookmarks::BookContainerKind::Pdf,
                page_identity: crate::book_bookmarks::PageIdentity::PdfPage(4),
                page_index_hint: 4,
                created_at_ms: 1,
                title: Some("伏線".to_string()),
            }),
            item: GridItem::PdfPage {
                pdf_path: path,
                page_num: 4,
                content_type: None,
            },
            relative_page_provenance: None,
            image_meta: None,
            marker_thumbnail: None,
            created_at_ms: 1,
            missing: false,
        };

        assert_eq!(row.title(), Some("伏線"));
        assert_eq!(row.display_name(), "伏線 — 5 ページ");
        assert_eq!(row.details_name(), "story.pdf — 伏線");
    }

    #[test]
    fn details_name_keeps_media_filename_beside_bookmark_title() {
        let mut row = media_row(7, 1_000, "clip");
        let BookmarkRowSource::Media { title, .. } = &mut row.source else {
            unreachable!();
        };
        *title = Some("見どころ".to_string());

        assert_eq!(row.details_name(), "clip.mp4 — 見どころ");
    }

    #[test]
    fn grid_content_comparison_uses_stable_keys_and_detects_title_changes() {
        let current = vec![media_row(1, 1_000, "one"), media_row(2, 2_000, "two")];
        let mut reordered = vec![current[1].clone(), current[0].clone()];
        assert!(rows_have_same_grid_content(&current, &reordered));

        let BookmarkRowSource::Media { title, .. } = &mut reordered[0].source else {
            unreachable!();
        };
        *title = Some("更新後".to_string());
        assert!(!rows_have_same_grid_content(&current, &reordered));
    }
}
