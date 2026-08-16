use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use image::GenericImageView;
use mimageviewer_ipc::RemoteAddress;
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
    NetworkPath,
    Busy,
    NotFound,
    StaleGeneration(String),
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
            ResolveError::InvalidPath => Self::BadRequest,
            ResolveError::NetworkPath => Self::NetworkPath,
            ResolveError::Unavailable => Self::NotFound,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct FavoriteRoot {
    id: Uuid,
    name: String,
    path: PathBuf,
}

#[derive(Serialize)]
pub struct FavoritesResponse {
    favorites: Vec<FavoriteSummary>,
    remote_state_generation: String,
}

#[derive(Serialize)]
pub struct RemoteStateResponse {
    pub remote_state_generation: String,
}

#[derive(Serialize)]
struct FavoriteSummary {
    id: Uuid,
    name: String,
    path: String,
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

#[derive(Clone)]
struct LibrarySnapshot {
    favorites: Vec<FavoriteRoot>,
    sort_order: Option<String>,
}

struct ObservedDatabase {
    connection: Connection,
    data_version: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoredRotation {
    None,
    Cw90,
    Cw180,
    Cw270,
}

impl StoredRotation {
    fn from_degrees(degrees: i32) -> Self {
        match degrees.rem_euclid(360) {
            90 => Self::Cw90,
            180 => Self::Cw180,
            270 => Self::Cw270,
            _ => Self::None,
        }
    }

    fn swaps_dimensions(self) -> bool {
        matches!(self, Self::Cw90 | Self::Cw270)
    }

    fn apply(self, image: image::DynamicImage) -> image::DynamicImage {
        match self {
            Self::None => image,
            Self::Cw90 => image.rotate90(),
            Self::Cw180 => image.rotate180(),
            Self::Cw270 => image.rotate270(),
        }
    }
}

struct LibraryState {
    snapshot: Arc<LibrarySnapshot>,
    settings: Option<ObservedDatabase>,
    view_trim: Option<ObservedDatabase>,
    rotation: Option<ObservedDatabase>,
    generation_counter: u64,
    generation: String,
}

pub struct Library {
    state: Mutex<LibraryState>,
    generation_prefix: String,
    settings_path: Option<PathBuf>,
    view_trim_path: Option<PathBuf>,
    rotation_path: Option<PathBuf>,
}

impl Library {
    #[cfg(test)]
    pub fn empty_for_test(_cache_dir: PathBuf) -> Self {
        Self::from_test_favorites(Vec::new())
    }

    #[cfg(test)]
    fn from_test_favorites(favorites: Vec<FavoriteRoot>) -> Self {
        let prefix = "test".to_owned();
        Self {
            state: Mutex::new(LibraryState {
                snapshot: Arc::new(library_snapshot(favorites, None)),
                settings: None,
                view_trim: None,
                rotation: None,
                generation_counter: 1,
                generation: format!("{prefix}-1"),
            }),
            generation_prefix: prefix,
            settings_path: None,
            view_trim_path: None,
            rotation_path: None,
        }
    }

    #[cfg(test)]
    fn from_test_rotation_path(rotation_path: PathBuf) -> Self {
        let mut library = Self::from_test_favorites(Vec::new());
        library.rotation_path = Some(rotation_path);
        library
    }

    pub fn load(data_dir: &Path) -> Result<Self, StoreError> {
        let settings_path = data_dir.join("settings.db");
        let mut settings = open_observed_database(&settings_path)?.ok_or(StoreError::NotFound)?;
        let (data_version, favorites, sort_order) =
            read_stable_settings_snapshot(&settings.connection)?;
        settings.data_version = data_version;
        let view_trim_path = data_dir.join("view_trim.db");
        let view_trim = open_observed_database(&view_trim_path)?;
        let rotation_path = data_dir.join("rotation.db");
        let rotation = open_observed_database(&rotation_path)?;
        let mut generation_seed = [0_u8; 16];
        getrandom::fill(&mut generation_seed).map_err(|error| {
            StoreError::Io(std::io::Error::other(format!(
                "remote state generation seed failed: {error}"
            )))
        })?;
        let generation_prefix = Uuid::from_bytes(generation_seed).simple().to_string();
        Ok(Self {
            state: Mutex::new(LibraryState {
                snapshot: Arc::new(library_snapshot(favorites, sort_order)),
                settings: Some(settings),
                view_trim,
                rotation,
                generation_counter: 1,
                generation: format!("{generation_prefix}-1"),
            }),
            generation_prefix,
            settings_path: Some(settings_path),
            view_trim_path: Some(view_trim_path),
            rotation_path: Some(rotation_path),
        })
    }

    pub fn favorites(&self) -> Result<FavoritesResponse, StoreError> {
        let (snapshot, generation) = self.snapshot(false)?;
        Ok(FavoritesResponse {
            favorites: snapshot
                .favorites
                .iter()
                .map(|favorite| FavoriteSummary {
                    id: favorite.id,
                    name: favorite.name.clone(),
                    path: favorite.path.to_string_lossy().into_owned(),
                })
                .collect(),
            remote_state_generation: generation,
        })
    }

    pub fn remote_state(&self) -> Result<RemoteStateResponse, StoreError> {
        let (_, remote_state_generation) = self.snapshot(true)?;
        Ok(RemoteStateResponse {
            remote_state_generation,
        })
    }

    pub fn require_remote_state_generation(&self, expected: &str) -> Result<String, StoreError> {
        let (_, current) = self.snapshot(true)?;
        if expected == current {
            Ok(current)
        } else {
            Err(StoreError::StaleGeneration(current))
        }
    }

    pub fn validate_remote_address(&self, address: &RemoteAddress) -> Result<(), StoreError> {
        address.validate_syntax().map_err(|error| match error {
            mimageviewer_ipc::AddressError::NetworkPath => StoreError::NetworkPath,
            mimageviewer_ipc::AddressError::InvalidPath
            | mimageviewer_ipc::AddressError::InvalidZipPath => StoreError::BadRequest,
        })?;
        resolve_existing(&address.path)?;
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
        let path = resolve_existing(&address.path)?.canonical;
        let metadata = std::fs::metadata(&path)?;
        if metadata.is_file() && classify_path(&path) == EntryKind::Image {
            Ok(())
        } else {
            Err(StoreError::BadRequest)
        }
    }

    /// HTTP 層でも絶対パス・実在・種類を検証する。本体 IPC も独立に再検証する。
    pub(crate) fn validate_remote_file_streamable(
        &self,
        address: &RemoteAddress,
    ) -> Result<(), StoreError> {
        if !matches!(
            address.subresource,
            mimageviewer_ipc::RemoteSubresource::File
        ) {
            return Err(StoreError::BadRequest);
        }
        let path = resolve_existing(&address.path)?.canonical;
        let metadata = std::fs::metadata(&path)?;
        if metadata.is_file() && matches!(classify_path(&path), EntryKind::Video | EntryKind::Audio)
        {
            Ok(())
        } else {
            Err(StoreError::BadRequest)
        }
    }

    pub fn image(&self, path: &str, requested_width: u32) -> Result<ImageResult, StoreError> {
        if requested_width == 0 || requested_width > MAX_IMAGE_WIDTH {
            return Err(StoreError::BadRequest);
        }
        let resolved = resolve_existing(path)?;
        let rotation = self.image_rotation(&resolved.logical)?;
        let image_path = resolved.canonical;
        let metadata = require_image_file(&image_path)?;

        let probe = image_support::probe_image(&image_path).ok_or(StoreError::Decode)?;
        if rotation == StoredRotation::None
            && let Some(content_type) =
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
        let image =
            rotation.apply(image_support::decode_oriented(&image_path).ok_or(StoreError::Decode)?);
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

    pub fn image_info(&self, path: &str) -> Result<ImageInfoResponse, StoreError> {
        let resolved = resolve_existing(path)?;
        let rotation = self.image_rotation(&resolved.logical)?;
        let image_path = resolved.canonical;
        require_image_file(&image_path)?;
        let probe = image_support::probe_image(&image_path).ok_or(StoreError::Decode)?;
        let (mut width, mut height) = probe.oriented_dimensions();
        if rotation.swaps_dimensions() {
            std::mem::swap(&mut width, &mut height);
        }
        Ok(ImageInfoResponse { width, height })
    }

    fn image_rotation(&self, logical_path: &Path) -> Result<StoredRotation, StoreError> {
        let mut state = self.state.lock().map_err(|_| StoreError::BadRequest)?;
        if state.rotation.is_none()
            && let Some(path) = self.rotation_path.as_deref()
        {
            state.rotation = open_observed_database(path)?;
        }
        let Some(database) = state.rotation.as_ref() else {
            return Ok(StoredRotation::None);
        };
        let key = rotation_page_key(logical_path);
        match database.connection.query_row(
            "SELECT angle FROM rotations WHERE path = ?1",
            [&key],
            |row| row.get::<_, i32>(0),
        ) {
            Ok(degrees) => Ok(StoredRotation::from_degrees(degrees)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(StoredRotation::None),
            Err(error) => Err(StoreError::Db(error)),
        }
    }

    fn snapshot(
        &self,
        include_page_display_state: bool,
    ) -> Result<(Arc<LibrarySnapshot>, String), StoreError> {
        let mut state = self.state.lock().map_err(|_| StoreError::BadRequest)?;
        let mut changed = refresh_settings_snapshot(&mut state, self.settings_path.as_deref())?;
        if include_page_display_state {
            changed |=
                refresh_observed_database(&mut state.view_trim, self.view_trim_path.as_deref())?;
            changed |=
                refresh_observed_database(&mut state.rotation, self.rotation_path.as_deref())?;
        }
        if changed {
            state.generation_counter = state.generation_counter.saturating_add(1);
            state.generation = format!("{}-{}", self.generation_prefix, state.generation_counter);
        }
        Ok((Arc::clone(&state.snapshot), state.generation.clone()))
    }
}

/// `rotation.db` uses the same normal-file page key as the core process.
/// ZIP/PDF keys stay in core because the legacy `/api/image` route only accepts files.
fn rotation_page_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase().replace('\\', "/")
}

fn library_snapshot(favorites: Vec<FavoriteRoot>, sort_order: Option<String>) -> LibrarySnapshot {
    LibrarySnapshot {
        favorites,
        sort_order,
    }
}

fn refresh_settings_snapshot(
    state: &mut LibraryState,
    path: Option<&Path>,
) -> Result<bool, StoreError> {
    if state.settings.is_none() {
        let Some(path) = path else {
            return Ok(false);
        };
        state.settings = open_observed_database(path)?;
        if state.settings.is_none() {
            return Err(StoreError::NotFound);
        }
    }
    let settings = state
        .settings
        .as_mut()
        .expect("settings observer initialized");
    let current = sqlite_data_version(&settings.connection)?;
    if current == settings.data_version {
        return Ok(false);
    }
    let (data_version, favorites, sort_order) =
        read_stable_settings_snapshot(&settings.connection)?;
    settings.data_version = data_version;
    if favorites == state.snapshot.favorites && sort_order == state.snapshot.sort_order {
        return Ok(false);
    }
    state.snapshot = Arc::new(library_snapshot(favorites, sort_order));
    Ok(true)
}

fn refresh_observed_database(
    observed: &mut Option<ObservedDatabase>,
    path: Option<&Path>,
) -> Result<bool, StoreError> {
    if let Some(database) = observed.as_mut() {
        let current = sqlite_data_version(&database.connection)?;
        if current == database.data_version {
            return Ok(false);
        }
        database.data_version = current;
        return Ok(true);
    }
    let Some(path) = path else {
        return Ok(false);
    };
    let opened = open_observed_database(path)?;
    let changed = opened.is_some();
    *observed = opened;
    Ok(changed)
}

fn open_observed_database(path: &Path) -> Result<Option<ObservedDatabase>, StoreError> {
    if !path.try_exists()? {
        return Ok(None);
    }
    let connection = open_read_only(path)?;
    let data_version = sqlite_data_version(&connection)?;
    Ok(Some(ObservedDatabase {
        connection,
        data_version,
    }))
}

fn sqlite_data_version(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row("PRAGMA data_version", [], |row| row.get(0))
}

fn read_favorite_roots(connection: &Connection) -> Result<Vec<FavoriteRoot>, StoreError> {
    let mut stmt = connection.prepare(
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
    Ok(favorites)
}

fn read_sort_order_value(connection: &Connection) -> Result<Option<String>, StoreError> {
    let has_settings_kv = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'settings_kv')",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !has_settings_kv {
        return Ok(None);
    }
    match connection.query_row(
        "SELECT value FROM settings_kv WHERE key = 'sort_order'",
        [],
        |row| row.get(0),
    ) {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn read_stable_settings_snapshot(
    connection: &Connection,
) -> Result<(i64, Vec<FavoriteRoot>, Option<String>), StoreError> {
    for _ in 0..3 {
        let before = sqlite_data_version(connection)?;
        let favorites = read_favorite_roots(connection)?;
        let sort_order = read_sort_order_value(connection)?;
        let after = sqlite_data_version(connection)?;
        if before == after {
            return Ok((after, favorites, sort_order));
        }
    }
    Err(StoreError::Busy)
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
    use mimageviewer_ipc::RemoteSubresource;

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
    fn favorites_json_includes_absolute_paths_for_navigation() {
        let id = Uuid::from_u128(0x1234567890abcdef1234567890abcdef);
        let library = Library::from_test_favorites(vec![FavoriteRoot {
            id,
            name: "Private library".to_owned(),
            path: PathBuf::from("C:/Users/example/Private"),
        }]);
        let json = serde_json::to_string(&library.favorites().unwrap()).unwrap();
        assert!(json.contains(&id.to_string()));
        assert!(json.contains("Private library"));
        assert!(json.contains("C:/Users/example/Private"));
    }

    #[test]
    fn validation_accepts_any_existing_absolute_path_and_keeps_subresource_checks() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside.jpg");
        std::fs::write(&outside, b"outside").unwrap();
        let library = Library::from_test_favorites(Vec::new());
        assert!(
            library
                .validate_remote_address(&RemoteAddress::file(
                    outside.to_string_lossy().into_owned(),
                ))
                .is_ok()
        );
        assert!(matches!(
            library.validate_remote_address(&RemoteAddress::file("outside.jpg")),
            Err(StoreError::BadRequest)
        ));
        let traversal = RemoteAddress {
            path: outside.to_string_lossy().into_owned(),
            subresource: RemoteSubresource::ZipEntry {
                entry_name: "../secret.jpg".to_owned(),
            },
        };
        assert!(matches!(
            library.validate_remote_address(&traversal),
            Err(StoreError::BadRequest)
        ));
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

        let favorites = library.favorites().unwrap();
        assert_eq!(favorites.favorites.len(), 1);
        assert_eq!(favorites.favorites[0].name, "Fixture");
        assert_eq!(favorites.favorites[0].path, favorite_root.to_string_lossy());
        let favorites_json = serde_json::to_string(&favorites).unwrap();
        assert!(favorites_json.contains("Fixture"));
        let traversal = RemoteAddress {
            path: "../page.png".to_owned(),
            subresource: RemoteSubresource::File,
        };
        assert!(matches!(
            library.validate_remote_address(&traversal),
            Err(StoreError::BadRequest)
        ));

        let image_path_text = image_path.to_string_lossy().into_owned();
        let resized_webp = library.image(&image_path_text, 10).unwrap();
        let resized = image::load_from_memory(&resized_webp.bytes).unwrap();
        assert_eq!(resized.dimensions(), (10, 5));
        assert_eq!(resized_webp.metrics.source_width, 40);
        assert_eq!(resized_webp.metrics.output_width, 10);
        assert_eq!(resized_webp.metrics.source_bytes, image_metadata.len());
        assert!(!resized_webp.metrics.passthrough);
        let passthrough = library.image(&image_path_text, 40).unwrap();
        assert_eq!(passthrough.content_type, "image/png");
        assert_eq!(passthrough.bytes, std::fs::read(&image_path).unwrap());
        assert!(passthrough.metrics.passthrough);
        assert_eq!(passthrough.metrics.decode_ms, 0.0);
        assert_eq!(passthrough.metrics.webp_encode_ms, 0.0);

        assert_eq!(std::fs::read(&settings_path).unwrap(), settings_before);
    }

    #[test]
    fn legacy_file_image_routes_follow_all_saved_quarter_turns() {
        let temp = tempfile::tempdir().unwrap();
        let image_path = temp.path().join("page.png");
        RgbaImage::from_fn(3, 2, |x, y| {
            Rgba([
                (x * 70) as u8,
                (y * 110) as u8,
                (x * 30 + y * 20) as u8,
                255,
            ])
        })
        .save(&image_path)
        .unwrap();

        let rotation_path = temp.path().join("rotation.db");
        let writer = Connection::open(&rotation_path).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE rotations (
                    path TEXT PRIMARY KEY,
                    angle INTEGER NOT NULL DEFAULT 0
                 );",
            )
            .unwrap();
        let logical = resolve_existing(image_path.to_string_lossy().as_ref())
            .unwrap()
            .logical;
        let key = rotation_page_key(&logical);
        let library = Library::from_test_rotation_path(rotation_path);
        let image_path_text = image_path.to_string_lossy().into_owned();

        for (degrees, expected_dimensions, expected_passthrough) in [
            (0, (3, 2), true),
            (90, (2, 3), false),
            (180, (3, 2), false),
            (270, (2, 3), false),
        ] {
            writer
                .execute(
                    "INSERT INTO rotations (path, angle) VALUES (?1, ?2)
                     ON CONFLICT(path) DO UPDATE SET angle = ?2",
                    rusqlite::params![key, degrees],
                )
                .unwrap();

            let info = library.image_info(&image_path_text).unwrap();
            assert_eq!((info.width, info.height), expected_dimensions);
            let result = library.image(&image_path_text, 3).unwrap();
            assert_eq!(
                (result.metrics.source_width, result.metrics.source_height),
                expected_dimensions
            );
            assert_eq!(result.metrics.passthrough, expected_passthrough);
            assert_eq!(
                image::load_from_memory(&result.bytes).unwrap().dimensions(),
                expected_dimensions
            );
        }
    }

    #[test]
    fn live_library_refreshes_favorites_without_changing_path_access() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let first_root = temp.path().join("first");
        let second_root = temp.path().join("second");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&first_root).unwrap();
        std::fs::create_dir_all(&second_root).unwrap();
        std::fs::write(first_root.join("page.jpg"), b"first").unwrap();
        std::fs::write(second_root.join("page.jpg"), b"second").unwrap();
        let first_id = Uuid::from_u128(1);
        let second_id = Uuid::from_u128(2);
        let settings_path = data_dir.join("settings.db");
        let writer = Connection::open(&settings_path).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE favorites (
                    id BLOB PRIMARY KEY,
                    name TEXT NOT NULL,
                    path TEXT NOT NULL,
                    sort_index INTEGER NOT NULL
                 );",
            )
            .unwrap();
        writer
            .execute(
                "INSERT INTO favorites VALUES (?1, 'First', ?2, 0)",
                rusqlite::params![
                    first_id.as_bytes().as_slice(),
                    first_root.to_string_lossy().as_ref()
                ],
            )
            .unwrap();

        let library = Library::load(&data_dir).unwrap();
        let initial = library.favorites().unwrap();
        assert_eq!(initial.favorites[0].name, "First");

        writer
            .execute(
                "INSERT INTO favorites VALUES (?1, 'Second renamed', ?2, 0)",
                rusqlite::params![
                    second_id.as_bytes().as_slice(),
                    second_root.to_string_lossy().as_ref()
                ],
            )
            .unwrap();
        writer
            .execute(
                "UPDATE favorites SET name = 'First renamed', sort_index = 1 WHERE id = ?1",
                [first_id.as_bytes().as_slice()],
            )
            .unwrap();
        let refreshed = library.favorites().unwrap();
        assert_ne!(
            refreshed.remote_state_generation,
            initial.remote_state_generation
        );
        assert_eq!(
            refreshed
                .favorites
                .iter()
                .map(|favorite| favorite.name.as_str())
                .collect::<Vec<_>>(),
            ["Second renamed", "First renamed"]
        );
        assert!(
            library
                .validate_remote_address(&RemoteAddress::file(
                    second_root.join("page.jpg").to_string_lossy().into_owned(),
                ))
                .is_ok()
        );

        writer
            .execute(
                "DELETE FROM favorites WHERE id = ?1",
                [first_id.as_bytes().as_slice()],
            )
            .unwrap();
        assert!(
            library
                .validate_remote_address(&RemoteAddress::file(
                    first_root.join("page.jpg").to_string_lossy().into_owned(),
                ))
                .is_ok()
        );
    }

    #[test]
    fn one_generation_observes_favorites_view_trim_and_rotation_but_ignores_unrelated_settings() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path();
        let settings_path = data_dir.join("settings.db");
        let settings_writer = Connection::open(&settings_path).unwrap();
        settings_writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE favorites (
                    id BLOB PRIMARY KEY,
                    name TEXT NOT NULL,
                    path TEXT NOT NULL,
                    sort_index INTEGER NOT NULL
                 );
                 CREATE TABLE settings_kv (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                 );
                 INSERT INTO settings_kv VALUES ('sort_order', 'FileName');
                 CREATE TABLE unrelated (value INTEGER NOT NULL);",
            )
            .unwrap();
        let favorite_id = Uuid::from_u128(1);
        settings_writer
            .execute(
                "INSERT INTO favorites VALUES (?1, 'First', ?2, 0)",
                rusqlite::params![
                    favorite_id.as_bytes().as_slice(),
                    data_dir.to_string_lossy()
                ],
            )
            .unwrap();

        let view_trim_path = data_dir.join("view_trim.db");
        let view_trim_writer = Connection::open(&view_trim_path).unwrap();
        view_trim_writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE view_trim_state (value INTEGER NOT NULL);",
            )
            .unwrap();

        let rotation_path = data_dir.join("rotation.db");
        let rotation_writer = Connection::open(&rotation_path).unwrap();
        rotation_writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE rotations (
                    path TEXT PRIMARY KEY,
                    angle INTEGER NOT NULL
                 );",
            )
            .unwrap();

        let library = Library::load(data_dir).unwrap();
        let initial = library.remote_state().unwrap().remote_state_generation;

        settings_writer
            .execute("INSERT INTO unrelated VALUES (1)", [])
            .unwrap();
        assert_eq!(
            library.remote_state().unwrap().remote_state_generation,
            initial,
            "unrelated settings writes must not invalidate page resources"
        );

        settings_writer
            .execute(
                "UPDATE settings_kv SET value = 'DateDesc' WHERE key = 'sort_order'",
                [],
            )
            .unwrap();
        let after_sort = library.remote_state().unwrap().remote_state_generation;
        assert_ne!(after_sort, initial);

        view_trim_writer
            .execute("INSERT INTO view_trim_state VALUES (1)", [])
            .unwrap();
        let after_trim = library.remote_state().unwrap().remote_state_generation;
        assert_ne!(after_trim, after_sort);
        assert!(matches!(
            library.require_remote_state_generation(&initial),
            Err(StoreError::StaleGeneration(current)) if current == after_trim
        ));

        settings_writer
            .execute(
                "UPDATE favorites SET name = 'Renamed' WHERE id = ?1",
                [favorite_id.as_bytes().as_slice()],
            )
            .unwrap();
        let after_favorite = library.remote_state().unwrap().remote_state_generation;
        assert_ne!(after_favorite, after_trim);

        rotation_writer
            .execute(
                "INSERT INTO rotations (path, angle) VALUES (?1, 90)",
                [rotation_page_key(&data_dir.join("page.jpg"))],
            )
            .unwrap();
        let after_rotation = library.remote_state().unwrap().remote_state_generation;
        assert_ne!(after_rotation, after_favorite);
    }
}
