use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

use image::GenericImageView;
use mimageviewer_ipc::{ContainerEntry, RemoteAddress, RemoteEntry};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
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

#[derive(Serialize)]
pub struct ListResponse {
    favorite_id: Uuid,
    path: String,
    thumb_aspect_height_ratio: f64,
    entries: Vec<ListEntry>,
}

pub struct ListResult {
    pub response: ListResponse,
    pub metrics: ListMetrics,
}

#[derive(Serialize)]
pub struct ListMetrics {
    pub entry_count: usize,
    pub scanned_count: usize,
    pub scan_ms: f64,
}

#[derive(Serialize)]
pub struct ListEntry {
    kind: EntryKind,
    name: String,
    path: String,
    size: u64,
    mtime: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
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
    thumb_aspect: StoredThumbAspect,
    thumb_aspect_auto: bool,
    auto_aspect_cache_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
enum StoredThumbAspect {
    Landscape16x9,
    Landscape3x2,
    Landscape4x3,
    #[default]
    Square,
    Portrait3x4,
    Portrait2x3,
    Portrait9x16,
}

fn thumb_aspect_height_ratio(aspect: StoredThumbAspect) -> f64 {
    match aspect {
        StoredThumbAspect::Landscape16x9 => 9.0 / 16.0,
        StoredThumbAspect::Landscape3x2 => 2.0 / 3.0,
        StoredThumbAspect::Landscape4x3 => 3.0 / 4.0,
        StoredThumbAspect::Square => 1.0,
        StoredThumbAspect::Portrait3x4 => 4.0 / 3.0,
        StoredThumbAspect::Portrait2x3 => 3.0 / 2.0,
        StoredThumbAspect::Portrait9x16 => 16.0 / 9.0,
    }
}

fn thumb_aspect_from_cache_int(value: i32) -> Option<StoredThumbAspect> {
    // Keep this explicit mapping in lockstep with src/auto_aspect_cache.rs's
    // aspect_to_int/aspect_from_int pair. Do not rely on Rust enum layout.
    match value {
        0 => Some(StoredThumbAspect::Landscape16x9),
        1 => Some(StoredThumbAspect::Landscape3x2),
        2 => Some(StoredThumbAspect::Landscape4x3),
        3 => Some(StoredThumbAspect::Square),
        4 => Some(StoredThumbAspect::Portrait3x4),
        5 => Some(StoredThumbAspect::Portrait2x3),
        6 => Some(StoredThumbAspect::Portrait9x16),
        _ => None,
    }
}

fn resolve_thumb_aspect_height_ratio(
    manual: StoredThumbAspect,
    auto: bool,
    cached: Option<StoredThumbAspect>,
) -> f64 {
    let effective = if auto {
        // This is App::effective_thumb_aspect's unresolved Auto fallback.
        cached.unwrap_or(StoredThumbAspect::Square)
    } else {
        manual
    };
    thumb_aspect_height_ratio(effective)
}

impl Library {
    #[cfg(test)]
    pub fn empty_for_test(_cache_dir: PathBuf) -> Self {
        Self {
            favorites: Vec::new(),
            by_id: HashMap::new(),
            thumb_aspect: StoredThumbAspect::Square,
            thumb_aspect_auto: false,
            auto_aspect_cache_path: PathBuf::from("auto_aspect_cache.db"),
        }
    }

    pub fn load(data_dir: &Path) -> Result<Self, StoreError> {
        let settings_path = data_dir.join("settings.db");
        let conn = open_read_only(&settings_path)?;
        let thumb_aspect =
            read_setting_json::<StoredThumbAspect>(&conn, "thumb_aspect")?.unwrap_or_default();
        let thumb_aspect_auto =
            read_setting_json::<bool>(&conn, "thumb_aspect_auto")?.unwrap_or(false);
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

        Ok(Self {
            favorites,
            by_id,
            thumb_aspect,
            thumb_aspect_auto,
            auto_aspect_cache_path: data_dir.join("auto_aspect_cache.db"),
        })
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

    pub fn validate_remote_address(&self, address: &RemoteAddress) -> Result<(), StoreError> {
        address
            .validate_syntax()
            .map_err(|_| StoreError::BadRequest)?;
        let id = Uuid::parse_str(&address.favorite_id).map_err(|_| StoreError::BadRequest)?;
        let favorite = self.favorite(id)?;
        resolve_existing(&favorite.path, &address.relative_path)?;
        Ok(())
    }

    pub fn retain_allowed_container_entries(&self, entries: &mut Vec<ContainerEntry>) {
        entries.retain(|entry| self.validate_remote_address(&entry.address).is_ok());
    }

    pub fn list(&self, favorite_id: Uuid, relative: &str) -> Result<ListResult, StoreError> {
        let favorite = self.favorite(favorite_id)?;
        let directory = resolve_existing(&favorite.path, relative)?;
        let metadata = std::fs::metadata(&directory)?;
        if !metadata.is_dir() {
            return Err(StoreError::BadRequest);
        }
        let logical_directory = logical_folder_path(&favorite.path, relative);
        let cached_aspect = if self.thumb_aspect_auto {
            read_cached_thumb_aspect(&self.auto_aspect_cache_path, &logical_directory)?
        } else {
            None
        };
        let thumb_aspect_height_ratio = resolve_thumb_aspect_height_ratio(
            self.thumb_aspect,
            self.thumb_aspect_auto,
            cached_aspect,
        );

        let mut entries = Vec::new();
        let mut scanned_count = 0;
        let scan_started = Instant::now();
        for entry_result in std::fs::read_dir(&directory)? {
            let entry = entry_result?;
            scanned_count += 1;
            // DirEntry::file_type uses the FindFirstFile data on Windows. Do not
            // replace this with per-entry Path::is_dir/is_file calls.
            let file_type = entry.file_type()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child_relative = join_relative(relative, &name);
            let entry_metadata = entry.metadata()?;

            let metadata = if file_type.is_symlink() || is_reparse_point(&entry_metadata) {
                // Links and Windows reparse points need a canonical containment
                // check before even metadata is published. DirEntry metadata is
                // cached on Windows, so regular entries gain no extra syscall.
                let resolved = match resolve_existing(&favorite.path, &child_relative) {
                    Ok(path) => path,
                    Err(_) => continue,
                };
                std::fs::metadata(&resolved)?
            } else if file_type.is_dir() || file_type.is_file() {
                entry_metadata
            } else {
                // Special filesystem entries are not exposed by the PoC.
                continue;
            };
            let effective_is_dir = file_type.is_dir() || metadata.is_dir();
            let effective_is_file = file_type.is_file() || metadata.is_file();
            let kind = classify_entry(&name, effective_is_dir, effective_is_file);
            entries.push(ListEntry {
                kind,
                name,
                path: child_relative,
                size: if effective_is_dir { 0 } else { metadata.len() },
                mtime: mtime_secs(&metadata),
            });
        }
        let scan_ms = duration_ms(scan_started.elapsed());

        entries.sort_by(|left, right| {
            sort_group(left.kind)
                .cmp(&sort_group(right.kind))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.name.cmp(&right.name))
        });

        let entry_count = entries.len();
        Ok(ListResult {
            response: ListResponse {
                favorite_id,
                path: normalize_relative_for_url(relative),
                thumb_aspect_height_ratio,
                entries,
            },
            metrics: ListMetrics {
                entry_count,
                scanned_count,
                scan_ms,
            },
        })
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

fn read_setting_json<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    key: &str,
) -> Result<Option<T>, StoreError> {
    let raw = connection
        .query_row(
            "SELECT value FROM settings_kv WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(raw.and_then(|value| serde_json::from_str(&value).ok()))
}

fn read_cached_thumb_aspect(
    cache_path: &Path,
    logical_folder: &Path,
) -> Result<Option<StoredThumbAspect>, StoreError> {
    match std::fs::metadata(cache_path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(StoreError::Io(error)),
    }
    let connection = open_read_only(cache_path)?;
    let folder_key = normalize_keep_drive(logical_folder);
    let raw = connection
        .query_row(
            "SELECT aspect FROM auto_aspect_cache WHERE folder_key = ?1",
            params![folder_key],
            |row| row.get::<_, i32>(0),
        )
        .optional()?;
    Ok(raw.and_then(thumb_aspect_from_cache_int))
}

fn logical_folder_path(root: &Path, relative: &str) -> PathBuf {
    if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    }
}

fn normalize_keep_drive(path: &Path) -> String {
    path.to_string_lossy().to_lowercase().replace('\\', "/")
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

fn sort_group(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::Dir => 0,
        EntryKind::Image => 1,
        _ => 2,
    }
}

fn join_relative(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{}/{}", normalize_relative_for_url(parent), name)
    }
}

fn normalize_relative_for_url(relative: &str) -> String {
    relative
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>()
        .join("/")
}

fn mtime_secs(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(windows)]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

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
    fn thumb_aspect_cache_mapping_and_resolution_match_miv_sources() {
        let cases = [
            (0, StoredThumbAspect::Landscape16x9, 9.0 / 16.0),
            (1, StoredThumbAspect::Landscape3x2, 2.0 / 3.0),
            (2, StoredThumbAspect::Landscape4x3, 3.0 / 4.0),
            (3, StoredThumbAspect::Square, 1.0),
            (4, StoredThumbAspect::Portrait3x4, 4.0 / 3.0),
            (5, StoredThumbAspect::Portrait2x3, 3.0 / 2.0),
            (6, StoredThumbAspect::Portrait9x16, 16.0 / 9.0),
        ];
        for (raw, aspect, expected) in cases {
            assert_eq!(thumb_aspect_from_cache_int(raw), Some(aspect));
            let manual = resolve_thumb_aspect_height_ratio(aspect, false, None);
            let automatic =
                resolve_thumb_aspect_height_ratio(StoredThumbAspect::Square, true, Some(aspect));
            assert!((manual - expected).abs() < f64::EPSILON);
            assert!((automatic - expected).abs() < f64::EPSILON);
        }
        assert_eq!(thumb_aspect_from_cache_int(-1), None);
        assert_eq!(thumb_aspect_from_cache_int(7), None);
        assert_eq!(
            resolve_thumb_aspect_height_ratio(StoredThumbAspect::Portrait9x16, true, None),
            1.0
        );
        assert_eq!(
            resolve_thumb_aspect_height_ratio(
                StoredThumbAspect::Landscape3x2,
                false,
                Some(StoredThumbAspect::Portrait9x16),
            ),
            2.0 / 3.0
        );
    }

    #[test]
    fn auto_aspect_folder_key_keeps_drive_and_matches_logical_folder() {
        let root = Path::new(r"C:\Books\Series");
        assert_eq!(normalize_keep_drive(root), "c:/books/series");
        assert_eq!(
            normalize_keep_drive(&logical_folder_path(root, "Volume 01/Pages")),
            "c:/books/series/volume 01/pages"
        );
        assert_eq!(logical_folder_path(root, ""), root);
    }

    #[test]
    fn missing_auto_aspect_cache_falls_back_without_creating_a_database() {
        let temp = tempfile::tempdir().unwrap();
        let cache_path = temp.path().join("auto_aspect_cache.db");
        assert_eq!(
            read_cached_thumb_aspect(&cache_path, Path::new(r"C:\Books\Series")).unwrap(),
            None
        );
        assert!(!cache_path.exists());
        assert_eq!(
            resolve_thumb_aspect_height_ratio(StoredThumbAspect::Landscape16x9, true, None),
            1.0
        );
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
            thumb_aspect: StoredThumbAspect::Square,
            thumb_aspect_auto: false,
            auto_aspect_cache_path: PathBuf::from("auto_aspect_cache.db"),
        };
        let json = serde_json::to_string(&library.favorites()).unwrap();
        assert!(json.contains(&id.to_string()));
        assert!(json.contains("Private library"));
        assert!(!json.contains("C:/"));
        assert!(!json.contains("Users"));
    }

    #[test]
    fn listing_does_not_publish_a_link_that_escapes_the_favorite() {
        let temp = tempfile::tempdir().unwrap();
        let favorite_root = temp.path().join("favorite");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&favorite_root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.jpg"), b"secret").unwrap();
        let link = favorite_root.join("escape");

        if let Err(error) = make_dir_link(&outside, &link) {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                eprintln!("link creation is unavailable; listing assertion skipped");
                return;
            }
            panic!("failed to create test link: {error}");
        }

        let id = Uuid::from_u128(0xfedcba9876543210fedcba9876543210);
        let library = Library {
            favorites: vec![FavoriteRoot {
                id,
                name: "Fixture".to_owned(),
                path: favorite_root,
            }],
            by_id: HashMap::from([(id, 0)]),
            thumb_aspect: StoredThumbAspect::Square,
            thumb_aspect_auto: false,
            auto_aspect_cache_path: temp.path().join("auto_aspect_cache.db"),
        };
        let listing = library.list(id, "").unwrap();
        assert!(
            listing
                .response
                .entries
                .iter()
                .all(|entry| entry.name != "escape")
        );
        assert!(matches!(
            library.list(id, "escape"),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn library_reads_settings_aspect_and_image_without_db_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let favorite_root = temp.path().join("favorite");
        std::fs::create_dir_all(favorite_root.join("album")).unwrap();
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
                 );
                 CREATE TABLE settings_kv (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                 );
                 INSERT INTO settings_kv (key, value)
                 VALUES ('thumb_aspect', '\"Landscape3x2\"');
                 INSERT INTO settings_kv (key, value)
                 VALUES ('thumb_aspect_auto', 'true');",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO favorites (id, name, path, sort_index)
                 VALUES (?1, ?2, ?3, 0)",
                params![
                    favorite_id.as_bytes().as_slice(),
                    "Fixture",
                    favorite_root.to_string_lossy().as_ref()
                ],
            )
            .unwrap();
        }

        let auto_aspect_cache_path = data_dir.join("auto_aspect_cache.db");
        {
            let conn = Connection::open(&auto_aspect_cache_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE auto_aspect_cache (
                    folder_key TEXT PRIMARY KEY,
                    aspect INTEGER NOT NULL,
                    sample_count INTEGER NOT NULL DEFAULT 0,
                    eligible_total INTEGER NOT NULL DEFAULT 0,
                    updated_at INTEGER NOT NULL
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO auto_aspect_cache
                 (folder_key, aspect, sample_count, eligible_total, updated_at)
                 VALUES (?1, 0, 12, 20, 1)",
                params![normalize_keep_drive(&favorite_root)],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO auto_aspect_cache
                 (folder_key, aspect, sample_count, eligible_total, updated_at)
                 VALUES (?1, 5, 12, 20, 1)",
                params![normalize_keep_drive(&favorite_root.join("album"))],
            )
            .unwrap();
        }

        let settings_before = std::fs::read(&settings_path).unwrap();
        let auto_aspect_cache_before = std::fs::read(&auto_aspect_cache_path).unwrap();
        let library = Library::load(&data_dir).unwrap();

        let favorites_json = serde_json::to_string(&library.favorites()).unwrap();
        assert!(favorites_json.contains("Fixture"));
        assert!(!favorites_json.contains(&favorite_root.to_string_lossy().to_string()));

        let listing = library.list(favorite_id, "").unwrap();
        assert_eq!(listing.response.path, "");
        assert!((listing.response.thumb_aspect_height_ratio - (9.0 / 16.0)).abs() < f64::EPSILON);
        let nested_listing = library.list(favorite_id, "album").unwrap();
        assert!(
            (nested_listing.response.thumb_aspect_height_ratio - (3.0 / 2.0)).abs() < f64::EPSILON
        );
        assert!(listing.response.entries.iter().any(|entry| {
            entry.kind == EntryKind::Dir && entry.name == "album" && entry.path == "album"
        }));
        assert!(listing.response.entries.iter().any(|entry| {
            entry.kind == EntryKind::Image && entry.name == "page.png" && entry.path == "page.png"
        }));
        assert_eq!(listing.metrics.entry_count, 2);
        assert_eq!(listing.metrics.scanned_count, 2);
        assert!(matches!(
            library.list(favorite_id, ".."),
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
        assert_eq!(
            std::fs::read(&auto_aspect_cache_path).unwrap(),
            auto_aspect_cache_before
        );
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
