use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use image::GenericImageView;
use mimageviewer_ipc::{ContainerEntry, FolderListEntry, RemoteAddress, RemoteEntry};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use uuid::Uuid;

use crate::diagnostics::duration_ms;
use crate::image_support;
use crate::path_guard::{ResolveError, resolve_existing};

pub const MAX_IMAGE_WIDTH: u32 = 32768;
const IMAGE_WEBP_QUALITY: f32 = 82.0;

#[derive(Debug)]
pub enum StoreError {
    BadRequest,
    NotFound,
    Io(std::io::Error),
    Db(rusqlite::Error),
    Decode,
}

impl From<std::io::Error> for StoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Db(value)
    }
}

impl From<ResolveError> for StoreError {
    fn from(value: ResolveError) -> Self {
        match value {
            ResolveError::InvalidRelativePath => Self::BadRequest,
            ResolveError::Unavailable | ResolveError::EscapesFavorite => Self::NotFound,
        }
    }
}

#[derive(Clone)]
struct FavoriteRoot {
    id: Uuid,
    name: String,
    path: PathBuf,
}

#[derive(Serialize)]
pub struct FavoritesResponse {
    favorites: Vec<FavoriteSummary>,
}

#[derive(Serialize)]
struct FavoriteSummary {
    id: Uuid,
    name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    Image,
    Video,
    Audio,
    Zip,
    Pdf,
    Other,
}

pub struct ImageResult {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub metrics: ImageMetrics,
}

#[derive(Serialize)]
pub struct ImageInfoResponse {
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize)]
pub struct ImageMetrics {
    pub source_width: u32,
    pub source_height: u32,
    pub output_width: u32,
    pub output_height: u32,
    pub requested_width: u32,
    pub decode_ms: f64,
    pub resize_ms: f64,
    pub webp_encode_ms: f64,
    pub source_read_ms: f64,
    pub source_bytes: u64,
    pub passthrough: bool,
}

pub struct Library {
    favorites: Vec<FavoriteRoot>,
    by_id: HashMap<Uuid, usize>,
}

impl Library {
    #[cfg(test)]
    pub fn empty_for_test(_cache_dir: PathBuf) -> Self {
        Self {
            favorites: Vec::new(),
            by_id: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub fn with_favorite_for_test(id: Uuid, path: PathBuf) -> Self {
        Self {
            favorites: vec![FavoriteRoot {
                id,
                name: "Fixture".to_owned(),
                path,
            }],
            by_id: HashMap::from([(id, 0)]),
        }
    }

    pub fn load(data_dir: &Path) -> Result<Self, StoreError> {
        let settings_path = data_dir.join("settings.db");
        let conn = open_read_only(&settings_path)?;

        let mut stmt = conn.prepare(
            "SELECT id, name, path
             FROM favorites
             ORDER BY sort_index ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: Vec<u8> = row.get(0)?;
            let name: String = row.get(1)?;
            let path: String = row.get(2)?;
            Ok((id, name, path))
        })?;

        let mut favorites = Vec::new();
        for row in rows {
            let (id, name, path) = row?;
            let id = Uuid::from_slice(&id).map_err(|_| StoreError::BadRequest)?;
            favorites.push(FavoriteRoot {
                id,
                name,
                path: PathBuf::from(path),
            });
        }
        let by_id = favorites
            .iter()
            .enumerate()
            .map(|(idx, favorite)| (favorite.id, idx))
            .collect();

        Ok(Self { favorites, by_id })
    }

    pub fn favorites(&self) -> FavoritesResponse {
        FavoritesResponse {
            favorites: self
                .favorites
                .iter()
                .map(|favorite| FavoriteSummary {
                    id: favorite.id,
                    name: favorite.name.clone(),
                })
                .collect(),
        }
    }

    /// 本体 IPC の応答にも remote-web 側の同じ allowlist を重ねる多重防御。
    /// UUID 不明、絶対 / traversal path、または junction による root 外脱出を除外する。
    pub fn retain_allowed_remote_entries(&self, entries: &mut Vec<RemoteEntry>) {
        entries.retain(|entry| {
            let Ok(id) = Uuid::parse_str(&entry.favorite_id) else {
                return false;
            };
            let Ok(favorite) = self.favorite(id) else {
                return false;
            };
            resolve_existing(&favorite.path, &entry.relative_path).is_ok()
        });
    }

    /// FolderList はセル本体とサムネイル出所の両方を外側の境界で再検証する。
    pub fn retain_allowed_folder_list_entries(&self, entries: &mut Vec<FolderListEntry>) {
        entries.retain(|entry| {
            if self.validate_remote_address(&entry.address).is_err() {
                return false;
            }
            entry.thumbnail_address == entry.address
                || self
                    .validate_remote_address(&entry.thumbnail_address)
                    .is_ok()
        });
    }

    pub fn validate_remote_address(&self, address: &RemoteAddress) -> Result<(), StoreError> {
        address
            .validate_syntax()
            .map_err(|_| StoreError::BadRequest)?;
        let id = Uuid::parse_str(&address.favorite_id).map_err(|_| StoreError::BadRequest)?;
        let favorite = self.favorite(id)?;
        resolve_existing(&favorite.path, &address.relative_path)?;
        Ok(())
    }

    pub(crate) fn validate_remote_file_image(
        &self,
        address: &RemoteAddress,
    ) -> Result<(), StoreError> {
        if !matches!(
            address.subresource,
            mimageviewer_ipc::RemoteSubresource::File
        ) {
            return Err(StoreError::BadRequest);
        }
        let id = Uuid::parse_str(&address.favorite_id).map_err(|_| StoreError::BadRequest)?;
        let favorite = self.favorite(id)?;
        let path = resolve_existing(&favorite.path, &address.relative_path)?;
        let metadata = std::fs::metadata(&path)?;
        if metadata.is_file() && classify_path(&path) == EntryKind::Image {
            Ok(())
        } else {
            Err(StoreError::BadRequest)
        }
    }

    /// HTTP 層でも favorite allowlist と canonical containment を検証する。
    /// 本体 IPC は同じ不変条件を独立に再検証するため、これは多重防御の外側である。
    pub(crate) fn validate_remote_file_video(
        &self,
        address: &RemoteAddress,
    ) -> Result<(), StoreError> {
        if !matches!(
            address.subresource,
            mimageviewer_ipc::RemoteSubresource::File
        ) {
            return Err(StoreError::BadRequest);
        }
        let id = Uuid::parse_str(&address.favorite_id).map_err(|_| StoreError::BadRequest)?;
        let favorite = self.favorite(id)?;
        let path = resolve_existing(&favorite.path, &address.relative_path)?;
        let metadata = std::fs::metadata(&path)?;
        if metadata.is_file() && classify_path(&path) == EntryKind::Video {
            Ok(())
        } else {
            Err(StoreError::BadRequest)
        }
    }

    pub fn retain_allowed_container_entries(&self, entries: &mut Vec<ContainerEntry>) {
        entries.retain(|entry| self.validate_remote_address(&entry.address).is_ok());
    }

    pub fn image(
        &self,
        favorite_id: Uuid,
        relative: &str,
        requested_width: u32,
    ) -> Result<ImageResult, StoreError> {
        if requested_width == 0 || requested_width > MAX_IMAGE_WIDTH {
            return Err(StoreError::BadRequest);
        }
        let favorite = self.favorite(favorite_id)?;
        let image_path = resolve_existing(&favorite.path, relative)?;
        let metadata = require_image_file(&image_path)?;

        let probe = image_support::probe_image(&image_path).ok_or(StoreError::Decode)?;
        if let Some(content_type) =
            image_support::passthrough_content_type(&image_path, probe, requested_width)
        {
            let read_started = Instant::now();
            let bytes = std::fs::read(&image_path)?;
            let source_read_ms = duration_ms(read_started.elapsed());
            let (source_width, source_height) = probe.oriented_dimensions();
            return Ok(ImageResult {
                bytes,
                content_type,
                metrics: ImageMetrics {
                    source_width,
                    source_height,
                    output_width: source_width,
                    output_height: source_height,
                    requested_width,
                    decode_ms: 0.0,
                    resize_ms: 0.0,
                    webp_encode_ms: 0.0,
                    source_read_ms,
                    source_bytes: metadata.len(),
                    passthrough: true,
                },
            });
        }

        let decode_started = Instant::now();
        let image = image_support::decode_oriented(&image_path).ok_or(StoreError::Decode)?;
        let decode_ms = duration_ms(decode_started.elapsed());
        let (source_width, source_height) = image.dimensions();

        let resize_started = Instant::now();
        let resized = resize_to_width(&image, requested_width);
        let resize_ms = duration_ms(resize_started.elapsed());
        let (output_width, output_height) = resized.dimensions();

        let encode_started = Instant::now();
        let rgba = resized.to_rgba8();
        let encoder = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height());
        let bytes = encoder.encode(IMAGE_WEBP_QUALITY).to_vec();
        let webp_encode_ms = duration_ms(encode_started.elapsed());
        Ok(ImageResult {
            bytes,
            content_type: "image/webp",
            metrics: ImageMetrics {
                source_width,
                source_height,
                output_width,
                output_height,
                requested_width,
                decode_ms,
                resize_ms,
                webp_encode_ms,
                source_read_ms: 0.0,
                source_bytes: metadata.len(),
                passthrough: false,
            },
        })
    }

    pub fn image_info(
        &self,
        favorite_id: Uuid,
        relative: &str,
    ) -> Result<ImageInfoResponse, StoreError> {
        let favorite = self.favorite(favorite_id)?;
        let image_path = resolve_existing(&favorite.path, relative)?;
        require_image_file(&image_path)?;
        let probe = image_support::probe_image(&image_path).ok_or(StoreError::Decode)?;
        let (width, height) = probe.oriented_dimensions();
        Ok(ImageInfoResponse { width, height })
    }

    fn favorite(&self, id: Uuid) -> Result<&FavoriteRoot, StoreError> {
        self.by_id
            .get(&id)
            .and_then(|idx| self.favorites.get(*idx))
            .ok_or(StoreError::NotFound)
    }
}

fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.execute_batch("PRAGMA query_only=ON; PRAGMA busy_timeout=5000;")?;
    Ok(conn)
}

fn require_image_file(path: &Path) -> Result<std::fs::Metadata, StoreError> {
    let metadata = std::fs::metadata(path)?;
    if metadata.is_file() && classify_path(path) == EntryKind::Image {
        Ok(metadata)
    } else {
        Err(StoreError::NotFound)
    }
}

fn resize_to_width(image: &image::DynamicImage, requested_width: u32) -> image::DynamicImage {
    let (source_width, source_height) = image.dimensions();
    let target_width = requested_width.min(source_width).max(1);
    if target_width == source_width {
        return image.clone();
    }
    let target_height =
        ((source_height as f64 * target_width as f64 / source_width as f64).round() as u32).max(1);
    image_support::resize_exact(image, target_width, target_height)
}

fn classify_path(path: &Path) -> EntryKind {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    classify_entry(name, false, true)
}

pub fn classify_entry(name: &str, is_dir: bool, is_file: bool) -> EntryKind {
    if is_dir {
        return EntryKind::Dir;
    }
    if !is_file {
        return EntryKind::Other;
    }
    let extension = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    if image_support::SUPPORTED_IMAGE_EXTENSIONS.contains(&extension.as_str()) {
        EntryKind::Image
    } else if image_support::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        EntryKind::Video
    } else if image_support::SUPPORTED_AUDIO_EXTENSIONS.contains(&extension.as_str()) {
        EntryKind::Audio
    } else if matches!(extension.as_str(), "zip" | "cbz") {
        EntryKind::Zip
    } else if extension == "pdf" {
        EntryKind::Pdf
    } else {
        EntryKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};
    use mimageviewer_ipc::{RemoteEntryKind, RemoteSubresource};

    #[test]
    fn classifies_all_api_entry_kinds() {
        assert_eq!(classify_entry("album", true, false), EntryKind::Dir);
        for name in ["a.jpg", "a.PNG", "a.heic", "a.webp"] {
            assert_eq!(classify_entry(name, false, true), EntryKind::Image);
        }
        for name in ["a.mp4", "a.MKV", "a.wmv"] {
            assert_eq!(classify_entry(name, false, true), EntryKind::Video);
        }
        for name in ["a.mp3", "a.flac", "a.opus"] {
            assert_eq!(classify_entry(name, false, true), EntryKind::Audio);
        }
        for name in ["a.zip", "a.CBZ"] {
            assert_eq!(classify_entry(name, false, true), EntryKind::Zip);
        }
        assert_eq!(classify_entry("a.pdf", false, true), EntryKind::Pdf);
        assert_eq!(classify_entry("notes.txt", false, true), EntryKind::Other);
        assert_eq!(classify_entry("a.jpg", false, false), EntryKind::Other);
    }

    #[test]
    fn resizing_is_based_on_output_width() {
        let image = image::DynamicImage::new_rgba8(1200, 1800);
        let resized = resize_to_width(&image, 600);
        assert_eq!(resized.dimensions(), (600, 900));

        let small = image::DynamicImage::new_rgba8(320, 200);
        let unchanged = resize_to_width(&small, 1000);
        assert_eq!(unchanged.dimensions(), (320, 200));
    }

    #[test]
    fn read_only_connection_rejects_writes() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("settings.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute("CREATE TABLE favorites (id BLOB)", [])
                .unwrap();
        }
        let conn = open_read_only(&db_path).unwrap();
        let error = conn
            .execute("INSERT INTO favorites (id) VALUES (x'00')", [])
            .unwrap_err();
        assert!(matches!(error, rusqlite::Error::SqliteFailure(_, _)));
    }

    #[test]
    fn favorites_json_never_contains_configured_root_paths() {
        let id = Uuid::from_u128(0x1234567890abcdef1234567890abcdef);
        let library = Library {
            favorites: vec![FavoriteRoot {
                id,
                name: "Private library".to_owned(),
                path: PathBuf::from("C:/Users/example/Private"),
            }],
            by_id: HashMap::from([(id, 0)]),
        };
        let json = serde_json::to_string(&library.favorites()).unwrap();
        assert!(json.contains(&id.to_string()));
        assert!(json.contains("Private library"));
        assert!(!json.contains("C:/"));
        assert!(!json.contains("Users"));
    }

    #[test]
    fn folder_list_revalidates_cell_and_thumbnail_containment() {
        let temp = tempfile::tempdir().unwrap();
        let favorite_root = temp.path().join("favorite");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&favorite_root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(favorite_root.join("safe.jpg"), b"safe").unwrap();
        std::fs::write(outside.join("secret.jpg"), b"secret").unwrap();
        let link = favorite_root.join("escape");

        if let Err(error) = make_dir_link(&outside, &link) {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                eprintln!("link creation is unavailable; containment assertion skipped");
                return;
            }
            panic!("failed to create test link: {error}");
        }

        let id = Uuid::from_u128(0xfedcba9876543210fedcba9876543210);
        let library = Library::with_favorite_for_test(id, favorite_root);
        let address = |relative_path: &str| RemoteAddress {
            favorite_id: id.to_string(),
            relative_path: relative_path.to_owned(),
            subresource: RemoteSubresource::File,
        };
        let entry = |name: &str, cell: &str, thumbnail: &str| FolderListEntry {
            address: address(cell),
            thumbnail_address: address(thumbnail),
            name: name.to_owned(),
            kind: RemoteEntryKind::Image,
            size: 4,
            mtime: 0,
        };
        let mut entries = vec![
            entry("safe.jpg", "safe.jpg", "safe.jpg"),
            entry("bad-cell.jpg", "escape/secret.jpg", "safe.jpg"),
            entry("bad-thumbnail.jpg", "safe.jpg", "escape/secret.jpg"),
        ];

        library.retain_allowed_folder_list_entries(&mut entries);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "safe.jpg");
    }

    #[test]
    fn library_reads_favorites_and_image_without_db_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let favorite_root = temp.path().join("favorite");
        std::fs::create_dir_all(&favorite_root).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();

        let image_path = favorite_root.join("page.png");
        RgbaImage::from_pixel(40, 20, Rgba([30, 80, 160, 255]))
            .save(&image_path)
            .unwrap();
        let image_metadata = std::fs::metadata(&image_path).unwrap();
        let favorite_id = Uuid::from_u128(0xabcdef0123456789abcdef0123456789);

        let settings_path = data_dir.join("settings.db");
        {
            let conn = Connection::open(&settings_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE favorites (
                    id BLOB PRIMARY KEY,
                    name TEXT NOT NULL,
                    path TEXT NOT NULL,
                    sort_index INTEGER NOT NULL
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO favorites (id, name, path, sort_index)
                 VALUES (?1, ?2, ?3, 0)",
                rusqlite::params![
                    favorite_id.as_bytes().as_slice(),
                    "Fixture",
                    favorite_root.to_string_lossy().as_ref()
                ],
            )
            .unwrap();
        }

        let settings_before = std::fs::read(&settings_path).unwrap();
        let library = Library::load(&data_dir).unwrap();

        let favorites_json = serde_json::to_string(&library.favorites()).unwrap();
        assert!(favorites_json.contains("Fixture"));
        assert!(!favorites_json.contains(&favorite_root.to_string_lossy().to_string()));
        let traversal = RemoteAddress {
            favorite_id: favorite_id.to_string(),
            relative_path: "../page.png".to_owned(),
            subresource: RemoteSubresource::File,
        };
        assert!(matches!(
            library.validate_remote_address(&traversal),
            Err(StoreError::BadRequest)
        ));

        let resized_webp = library.image(favorite_id, "page.png", 10).unwrap();
        let resized = image::load_from_memory(&resized_webp.bytes).unwrap();
        assert_eq!(resized.dimensions(), (10, 5));
        assert_eq!(resized_webp.metrics.source_width, 40);
        assert_eq!(resized_webp.metrics.output_width, 10);
        assert_eq!(resized_webp.metrics.source_bytes, image_metadata.len());
        assert!(!resized_webp.metrics.passthrough);
        let passthrough = library.image(favorite_id, "page.png", 40).unwrap();
        assert_eq!(passthrough.content_type, "image/png");
        assert_eq!(passthrough.bytes, std::fs::read(&image_path).unwrap());
        assert!(passthrough.metrics.passthrough);
        assert_eq!(passthrough.metrics.decode_ms, 0.0);
        assert_eq!(passthrough.metrics.webp_encode_ms, 0.0);

        assert_eq!(std::fs::read(&settings_path).unwrap(), settings_before);
    }

    #[cfg(windows)]
    fn make_dir_link(target: &Path, link: &Path) -> std::io::Result<()> {
        match std::os::windows::fs::symlink_dir(target, link) {
            Ok(()) => Ok(()),
            Err(error) if error.raw_os_error() == Some(1314) => {
                let status = std::process::Command::new("cmd")
                    .args(["/d", "/c", "mklink", "/J"])
                    .arg(link)
                    .arg(target)
                    .status()?;
                if status.success() { Ok(()) } else { Err(error) }
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(unix)]
    fn make_dir_link(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }
}
