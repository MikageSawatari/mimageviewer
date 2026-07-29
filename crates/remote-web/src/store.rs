use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use image::GenericImageView;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use uuid::Uuid;

use crate::image_support;
use crate::path_guard::{ResolveError, resolve_existing};

pub const MAX_IMAGE_WIDTH: u32 = 4096;
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
    entries: Vec<ListEntry>,
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

pub struct Library {
    favorites: Vec<FavoriteRoot>,
    by_id: HashMap<Uuid, usize>,
    cache_dir: PathBuf,
}

impl Library {
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

        Ok(Self {
            favorites,
            by_id,
            cache_dir: data_dir.join("cache"),
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

    pub fn list(&self, favorite_id: Uuid, relative: &str) -> Result<ListResponse, StoreError> {
        let favorite = self.favorite(favorite_id)?;
        let directory = resolve_existing(&favorite.path, relative)?;
        let metadata = std::fs::metadata(&directory)?;
        if !metadata.is_dir() {
            return Err(StoreError::BadRequest);
        }

        let mut entries = Vec::new();
        for entry_result in std::fs::read_dir(&directory)? {
            let entry = entry_result?;
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

        entries.sort_by(|left, right| {
            sort_group(left.kind)
                .cmp(&sort_group(right.kind))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.name.cmp(&right.name))
        });

        Ok(ListResponse {
            favorite_id,
            path: normalize_relative_for_url(relative),
            entries,
        })
    }

    pub fn thumbnail(&self, favorite_id: Uuid, relative: &str) -> Result<Vec<u8>, StoreError> {
        let favorite = self.favorite(favorite_id)?;
        let image_path = resolve_existing(&favorite.path, relative)?;
        require_image_file(&image_path)?;

        // Catalog identity follows the configured/logical favorite path. The
        // canonical path above is only for the security boundary and file read;
        // its Windows verbatim spelling would miss the main application's DB.
        let logical_path = favorite.path.join(relative);
        let parent = logical_path.parent().ok_or(StoreError::NotFound)?;
        let filename = logical_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(StoreError::NotFound)?;
        let metadata = std::fs::metadata(&image_path)?;
        let catalog_path = image_support::catalog_db_path(&self.cache_dir, parent);
        if !catalog_path.try_exists().unwrap_or(false) {
            return Err(StoreError::NotFound);
        }
        let catalog = open_read_only(&catalog_path)?;
        let entry = catalog
            .query_row(
                "SELECT mtime, file_size, thumb_data
                 FROM thumbnails
                 WHERE filename = ?1",
                params![catalog_image_key(parent, &logical_path, filename)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or(StoreError::NotFound)?;
        if entry.0 != mtime_secs(&metadata) || entry.1 != metadata.len() as i64 {
            return Err(StoreError::NotFound);
        }
        if !is_webp(&entry.2) {
            return Err(StoreError::NotFound);
        }
        Ok(entry.2)
    }

    pub fn image(
        &self,
        favorite_id: Uuid,
        relative: &str,
        requested_width: u32,
    ) -> Result<Vec<u8>, StoreError> {
        if requested_width == 0 || requested_width > MAX_IMAGE_WIDTH {
            return Err(StoreError::BadRequest);
        }
        let favorite = self.favorite(favorite_id)?;
        let image_path = resolve_existing(&favorite.path, relative)?;
        require_image_file(&image_path)?;

        let image = image_support::decode_oriented(&image_path).ok_or(StoreError::Decode)?;
        let resized = resize_to_width(&image, requested_width);
        let rgba = resized.to_rgba8();
        let encoder = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height());
        Ok(encoder.encode(IMAGE_WEBP_QUALITY).to_vec())
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

fn require_image_file(path: &Path) -> Result<(), StoreError> {
    let metadata = std::fs::metadata(path)?;
    if metadata.is_file() && classify_path(path) == EntryKind::Image {
        Ok(())
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

fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
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

fn catalog_image_key(parent: &Path, logical_path: &Path, filename: &str) -> String {
    if parent.parent().is_none() {
        format!("imgthumb:{}", logical_path.to_string_lossy())
    } else {
        filename.to_owned()
    }
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
    fn webp_detection_does_not_mislabel_legacy_jpeg() {
        assert!(is_webp(b"RIFF\x10\x00\x00\x00WEBPdata"));
        assert!(!is_webp(b"\xff\xd8\xff\xe0legacy jpeg"));
    }

    #[test]
    fn drive_root_images_use_existing_full_path_cache_key_convention() {
        let root = Path::new("C:/");
        let image = root.join("cover.jpg");
        assert_eq!(
            catalog_image_key(root, &image, "cover.jpg"),
            "imgthumb:C:/cover.jpg"
        );
        assert_eq!(
            catalog_image_key(Path::new("C:/Photos"), &image, "cover.jpg"),
            "cover.jpg"
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
            cache_dir: PathBuf::from("cache"),
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
            cache_dir: temp.path().join("cache"),
        };
        let listing = library.list(id, "").unwrap();
        assert!(listing.entries.iter().all(|entry| entry.name != "escape"));
        assert!(matches!(
            library.list(id, "escape"),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn library_reads_temp_settings_catalog_and_image_without_db_mutation() {
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
                 );",
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

        let thumbnail = {
            let pixels = RgbaImage::from_pixel(8, 4, Rgba([30, 80, 160, 255]));
            webp::Encoder::from_rgba(pixels.as_raw(), 8, 4)
                .encode(75.0)
                .to_vec()
        };
        let catalog_path = image_support::catalog_db_path(&data_dir.join("cache"), &favorite_root);
        std::fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
        {
            let conn = Connection::open(&catalog_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE thumbnails (
                    filename TEXT NOT NULL PRIMARY KEY,
                    mtime INTEGER NOT NULL,
                    file_size INTEGER NOT NULL,
                    thumb_data BLOB NOT NULL
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO thumbnails (filename, mtime, file_size, thumb_data)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    "page.png",
                    mtime_secs(&image_metadata),
                    image_metadata.len() as i64,
                    &thumbnail
                ],
            )
            .unwrap();
        }

        let settings_before = std::fs::read(&settings_path).unwrap();
        let catalog_before = std::fs::read(&catalog_path).unwrap();
        let library = Library::load(&data_dir).unwrap();

        let favorites_json = serde_json::to_string(&library.favorites()).unwrap();
        assert!(favorites_json.contains("Fixture"));
        assert!(!favorites_json.contains(&favorite_root.to_string_lossy().to_string()));

        let listing = library.list(favorite_id, "").unwrap();
        assert_eq!(listing.path, "");
        assert!(listing.entries.iter().any(|entry| {
            entry.kind == EntryKind::Dir && entry.name == "album" && entry.path == "album"
        }));
        assert!(listing.entries.iter().any(|entry| {
            entry.kind == EntryKind::Image && entry.name == "page.png" && entry.path == "page.png"
        }));
        assert!(matches!(
            library.list(favorite_id, ".."),
            Err(StoreError::BadRequest)
        ));

        assert_eq!(
            library.thumbnail(favorite_id, "page.png").unwrap(),
            thumbnail
        );
        let resized_webp = library.image(favorite_id, "page.png", 10).unwrap();
        let resized = image::load_from_memory(&resized_webp).unwrap();
        assert_eq!(resized.dimensions(), (10, 5));

        assert_eq!(std::fs::read(&settings_path).unwrap(), settings_before);
        assert_eq!(std::fs::read(&catalog_path).unwrap(), catalog_before);
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
