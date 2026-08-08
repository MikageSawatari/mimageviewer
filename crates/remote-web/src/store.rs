use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use image::GenericImageView;
use mimageviewer_ipc::{ContainerEntry, FolderListEntry, RemoteAddress, RemoteEntry};
use mimageviewer_registered_roots::{RegisteredRootCatalog, RegisteredRootsSnapshot};
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
            ResolveError::InvalidRelativePath => Self::BadRequest,
            ResolveError::Unavailable | ResolveError::EscapesFavorite => Self::NotFound,
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
    by_id: HashMap<Uuid, usize>,
    registered: Arc<RegisteredRootsSnapshot>,
    sort_order: Option<String>,
}

struct ObservedDatabase {
    connection: Connection,
    data_version: i64,
}

struct LibraryState {
    snapshot: Arc<LibrarySnapshot>,
    settings: Option<ObservedDatabase>,
    view_trim: Option<ObservedDatabase>,
    registered: Option<RegisteredRootCatalog>,
    generation_counter: u64,
    generation: String,
}

pub struct Library {
    state: Mutex<LibraryState>,
    generation_prefix: String,
    settings_path: Option<PathBuf>,
    view_trim_path: Option<PathBuf>,
}

impl Library {
    #[cfg(test)]
    pub fn empty_for_test(_cache_dir: PathBuf) -> Self {
        Self::from_test_favorites(Vec::new())
    }

    #[cfg(test)]
    pub fn with_favorite_for_test(id: Uuid, path: PathBuf) -> Self {
        Self::from_test_favorites(vec![FavoriteRoot {
            id,
            name: "Fixture".to_owned(),
            path,
        }])
    }

    #[cfg(test)]
    fn from_test_favorites(favorites: Vec<FavoriteRoot>) -> Self {
        Self::from_test_roots(favorites, RegisteredRootsSnapshot::empty())
    }

    #[cfg(test)]
    fn with_registered_for_test(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self::from_test_roots(Vec::new(), RegisteredRootsSnapshot::from_paths(paths))
    }

    #[cfg(test)]
    fn from_test_roots(
        favorites: Vec<FavoriteRoot>,
        registered: Arc<RegisteredRootsSnapshot>,
    ) -> Self {
        let prefix = "test".to_owned();
        Self {
            state: Mutex::new(LibraryState {
                snapshot: Arc::new(library_snapshot(favorites, registered, None)),
                settings: None,
                view_trim: None,
                registered: None,
                generation_counter: 1,
                generation: format!("{prefix}-1"),
            }),
            generation_prefix: prefix,
            settings_path: None,
            view_trim_path: None,
        }
    }

    pub fn load(data_dir: &Path) -> Result<Self, StoreError> {
        let settings_path = data_dir.join("settings.db");
        let mut settings = open_observed_database(&settings_path)?.ok_or(StoreError::NotFound)?;
        let (data_version, favorites, sort_order) =
            read_stable_settings_snapshot(&settings.connection)?;
        settings.data_version = data_version;
        let view_trim_path = data_dir.join("view_trim.db");
        let view_trim = open_observed_database(&view_trim_path)?;
        let registered = RegisteredRootCatalog::open(data_dir).map_err(registered_store_error)?;
        let registered_snapshot = registered.snapshot();
        log_registered_limit(registered_snapshot.as_ref());
        let mut generation_seed = [0_u8; 16];
        getrandom::fill(&mut generation_seed).map_err(|error| {
            StoreError::Io(std::io::Error::other(format!(
                "remote state generation seed failed: {error}"
            )))
        })?;
        let generation_prefix = Uuid::from_bytes(generation_seed).simple().to_string();
        Ok(Self {
            state: Mutex::new(LibraryState {
                snapshot: Arc::new(library_snapshot(favorites, registered_snapshot, sort_order)),
                settings: Some(settings),
                view_trim,
                registered: Some(registered),
                generation_counter: 1,
                generation: format!("{generation_prefix}-1"),
            }),
            generation_prefix,
            settings_path: Some(settings_path),
            view_trim_path: Some(view_trim_path),
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

    /// 本体 IPC の応答にも remote-web 側の同じ allowlist を重ねる多重防御。
    /// UUID 不明、絶対 / traversal path、または junction による root 外脱出を除外する。
    pub fn retain_allowed_remote_entries(&self, entries: &mut Vec<RemoteEntry>) {
        let Ok((snapshot, _)) = self.snapshot(false) else {
            entries.clear();
            return;
        };
        entries.retain(|entry| {
            let Ok(id) = Uuid::parse_str(&entry.root_id) else {
                return false;
            };
            resolve_root_path_in(&snapshot, id, &entry.relative_path).is_ok()
        });
    }

    /// FolderList はセル本体とサムネイル出所の両方を外側の境界で再検証する。
    pub fn retain_allowed_folder_list_entries(&self, entries: &mut Vec<FolderListEntry>) {
        let Ok((snapshot, _)) = self.snapshot(false) else {
            entries.clear();
            return;
        };
        entries.retain(|entry| {
            if validate_remote_address_in(&snapshot, &entry.address).is_err() {
                return false;
            }
            entry.thumbnail_address == entry.address
                || validate_remote_address_in(&snapshot, &entry.thumbnail_address).is_ok()
        });
    }

    pub fn validate_remote_address(&self, address: &RemoteAddress) -> Result<(), StoreError> {
        let (snapshot, _) = self.snapshot(false)?;
        validate_remote_address_in(&snapshot, address)
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
        let id = Uuid::parse_str(&address.root_id).map_err(|_| StoreError::BadRequest)?;
        let path = self.resolve_root_path(id, &address.relative_path)?;
        let metadata = std::fs::metadata(&path)?;
        if metadata.is_file() && classify_path(&path) == EntryKind::Image {
            Ok(())
        } else {
            Err(StoreError::BadRequest)
        }
    }

    /// HTTP 層でも root allowlist と canonical containment を検証する。
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
        let id = Uuid::parse_str(&address.root_id).map_err(|_| StoreError::BadRequest)?;
        let path = self.resolve_root_path(id, &address.relative_path)?;
        let metadata = std::fs::metadata(&path)?;
        if metadata.is_file() && classify_path(&path) == EntryKind::Video {
            Ok(())
        } else {
            Err(StoreError::BadRequest)
        }
    }

    pub fn retain_allowed_container_entries(&self, entries: &mut Vec<ContainerEntry>) {
        let Ok((snapshot, _)) = self.snapshot(false) else {
            entries.clear();
            return;
        };
        entries.retain(|entry| validate_remote_address_in(&snapshot, &entry.address).is_ok());
    }

    pub fn image(
        &self,
        root_id: Uuid,
        relative: &str,
        requested_width: u32,
    ) -> Result<ImageResult, StoreError> {
        if requested_width == 0 || requested_width > MAX_IMAGE_WIDTH {
            return Err(StoreError::BadRequest);
        }
        let image_path = self.resolve_root_path(root_id, relative)?;
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
        root_id: Uuid,
        relative: &str,
    ) -> Result<ImageInfoResponse, StoreError> {
        let image_path = self.resolve_root_path(root_id, relative)?;
        require_image_file(&image_path)?;
        let probe = image_support::probe_image(&image_path).ok_or(StoreError::Decode)?;
        let (width, height) = probe.oriented_dimensions();
        Ok(ImageInfoResponse { width, height })
    }

    fn resolve_root_path(&self, id: Uuid, relative: &str) -> Result<PathBuf, StoreError> {
        let (snapshot, _) = self.snapshot(false)?;
        resolve_root_path_in(&snapshot, id, relative)
    }

    fn snapshot(
        &self,
        include_view_trim: bool,
    ) -> Result<(Arc<LibrarySnapshot>, String), StoreError> {
        let mut state = self.state.lock().map_err(|_| StoreError::BadRequest)?;
        let mut changed = refresh_settings_snapshot(&mut state, self.settings_path.as_deref())?;
        changed |= refresh_registered_roots(&mut state)?;
        if include_view_trim {
            changed |=
                refresh_observed_database(&mut state.view_trim, self.view_trim_path.as_deref())?;
        }
        if changed {
            state.generation_counter = state.generation_counter.saturating_add(1);
            state.generation = format!("{}-{}", self.generation_prefix, state.generation_counter);
        }
        Ok((Arc::clone(&state.snapshot), state.generation.clone()))
    }
}

fn library_snapshot(
    favorites: Vec<FavoriteRoot>,
    registered: Arc<RegisteredRootsSnapshot>,
    sort_order: Option<String>,
) -> LibrarySnapshot {
    let by_id = favorites
        .iter()
        .enumerate()
        .map(|(idx, favorite)| (favorite.id, idx))
        .collect();
    LibrarySnapshot {
        favorites,
        by_id,
        registered,
        sort_order,
    }
}

fn favorite_from_snapshot(snapshot: &LibrarySnapshot, id: Uuid) -> Option<&FavoriteRoot> {
    snapshot
        .by_id
        .get(&id)
        .and_then(|idx| snapshot.favorites.get(*idx))
}

fn validate_remote_address_in(
    snapshot: &LibrarySnapshot,
    address: &RemoteAddress,
) -> Result<(), StoreError> {
    address
        .validate_syntax()
        .map_err(|_| StoreError::BadRequest)?;
    let id = Uuid::parse_str(&address.root_id).map_err(|_| StoreError::BadRequest)?;
    resolve_root_path_in(snapshot, id, &address.relative_path)?;
    Ok(())
}

/// The only remote-web root resolver. Callers never branch on favorite versus registered roots.
fn resolve_root_path_in(
    snapshot: &LibrarySnapshot,
    id: Uuid,
    relative: &str,
) -> Result<PathBuf, StoreError> {
    if let Some(favorite) = favorite_from_snapshot(snapshot, id) {
        return resolve_existing(&favorite.path, relative).map_err(Into::into);
    }
    snapshot
        .registered
        .resolve_existing(id, relative)
        .map(|resolved| resolved.canonical)
        .map_err(|error| match error {
            mimageviewer_registered_roots::ResolveError::InvalidRelativePath
            | mimageviewer_registered_roots::ResolveError::FileRootHasRelativePath => {
                StoreError::BadRequest
            }
            mimageviewer_registered_roots::ResolveError::RootNotFound
            | mimageviewer_registered_roots::ResolveError::Unavailable
            | mimageviewer_registered_roots::ResolveError::EscapesRoot => StoreError::NotFound,
        })
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
    state.snapshot = Arc::new(library_snapshot(
        favorites,
        Arc::clone(&state.snapshot.registered),
        sort_order,
    ));
    Ok(true)
}

fn refresh_registered_roots(state: &mut LibraryState) -> Result<bool, StoreError> {
    let Some(registered) = state.registered.as_mut() else {
        return Ok(false);
    };
    if !registered.refresh().map_err(registered_store_error)? {
        return Ok(false);
    }
    let registered = registered.snapshot();
    log_registered_limit(registered.as_ref());
    state.snapshot = Arc::new(library_snapshot(
        state.snapshot.favorites.clone(),
        registered,
        state.snapshot.sort_order.clone(),
    ));
    Ok(true)
}

fn registered_store_error(error: mimageviewer_registered_roots::CatalogError) -> StoreError {
    match error {
        mimageviewer_registered_roots::CatalogError::Busy => StoreError::Busy,
        mimageviewer_registered_roots::CatalogError::Io(error) => StoreError::Io(error),
        mimageviewer_registered_roots::CatalogError::Database(error) => StoreError::Db(error),
        mimageviewer_registered_roots::CatalogError::InvalidSetting { .. } => {
            StoreError::BadRequest
        }
    }
}

fn log_registered_limit(snapshot: &RegisteredRootsSnapshot) {
    if snapshot.limit_reached() {
        eprintln!(
            "remote-web: registered root limit reached discovered={} retained={}",
            snapshot.discovered_count(),
            snapshot.roots().len()
        );
    }
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
        let library = Library::from_test_favorites(vec![FavoriteRoot {
            id,
            name: "Private library".to_owned(),
            path: PathBuf::from("C:/Users/example/Private"),
        }]);
        let json = serde_json::to_string(&library.favorites().unwrap()).unwrap();
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
            root_id: id.to_string(),
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
    fn double_validation_accepts_registered_granularity_and_subresources_only() {
        let temp = tempfile::tempdir().unwrap();
        let zip = temp.path().join("book.zip");
        let pdf = temp.path().join("book.pdf");
        let folder = temp.path().join("album");
        let child = folder.join("page.jpg");
        let unknown = temp.path().join("unknown.jpg");
        std::fs::write(&zip, b"zip").unwrap();
        std::fs::write(&pdf, b"pdf").unwrap();
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(&child, b"page").unwrap();
        std::fs::write(&unknown, b"unknown").unwrap();
        let library = Library::with_registered_for_test([zip.clone(), pdf.clone(), folder.clone()]);
        let zip_id = mimageviewer_registered_roots::registered_root_id(&zip).to_string();
        let pdf_id = mimageviewer_registered_roots::registered_root_id(&pdf).to_string();
        let folder_id = mimageviewer_registered_roots::registered_root_id(&folder).to_string();

        let zip_entry = RemoteAddress {
            root_id: zip_id.clone(),
            relative_path: String::new(),
            subresource: RemoteSubresource::ZipEntry {
                entry_name: "chapter/001.jpg".to_owned(),
            },
        };
        let pdf_page = RemoteAddress {
            root_id: pdf_id,
            relative_path: String::new(),
            subresource: RemoteSubresource::PdfPage { page_number: 0 },
        };
        assert!(library.validate_remote_address(&zip_entry).is_ok());
        assert!(library.validate_remote_address(&pdf_page).is_ok());
        assert!(
            library
                .validate_remote_address(&RemoteAddress::file(folder_id, "page.jpg"))
                .is_ok()
        );
        assert!(matches!(
            library.validate_remote_address(&RemoteAddress::file(zip_id.clone(), "sibling.jpg")),
            Err(StoreError::BadRequest)
        ));
        assert!(matches!(
            library.validate_remote_address(&RemoteAddress::file(
                mimageviewer_registered_roots::registered_root_id(temp.path()).to_string(),
                "book.zip"
            )),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            library.validate_remote_address(&RemoteAddress::file(
                mimageviewer_registered_roots::registered_root_id(&unknown).to_string(),
                ""
            )),
            Err(StoreError::NotFound)
        ));

        let remote_entry = |root_id: String, relative_path: &str, name: &str| RemoteEntry {
            root_id,
            relative_path: relative_path.to_owned(),
            name: name.to_owned(),
            kind: RemoteEntryKind::Zip,
            detail: None,
            progress_current: None,
            progress_total: None,
            rating: None,
        };
        let mut entries = vec![
            remote_entry(zip_id, "", "book.zip"),
            remote_entry(
                mimageviewer_registered_roots::registered_root_id(&unknown).to_string(),
                "",
                "unknown.jpg",
            ),
        ];
        library.retain_allowed_remote_entries(&mut entries);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "book.zip");
        let json = serde_json::to_string(&entries).unwrap();
        assert!(!json.contains(temp.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn live_double_validation_rejects_a_path_after_its_tag_is_removed() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let tagged = temp.path().join("tagged.jpg");
        std::fs::create_dir(&data_dir).unwrap();
        std::fs::write(&tagged, b"tagged").unwrap();
        Connection::open(data_dir.join("settings.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE favorites (
                    id BLOB PRIMARY KEY,
                    name TEXT NOT NULL,
                    path TEXT NOT NULL,
                    sort_index INTEGER NOT NULL
                );",
            )
            .unwrap();
        let tags = Connection::open(data_dir.join("tags.db")).unwrap();
        tags.execute_batch("CREATE TABLE item_tags (item_key TEXT NOT NULL);")
            .unwrap();
        tags.execute(
            "INSERT INTO item_tags VALUES (?1)",
            [tagged.to_string_lossy().as_ref()],
        )
        .unwrap();

        let library = Library::load(&data_dir).unwrap();
        let address = RemoteAddress::file(
            mimageviewer_registered_roots::registered_root_id(&tagged).to_string(),
            "",
        );
        assert!(library.validate_remote_address(&address).is_ok());
        let before = library.remote_state().unwrap().remote_state_generation;

        tags.execute("DELETE FROM item_tags", []).unwrap();

        assert!(matches!(
            library.validate_remote_address(&address),
            Err(StoreError::NotFound)
        ));
        assert_ne!(
            library.remote_state().unwrap().remote_state_generation,
            before
        );
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

        let favorites_json = serde_json::to_string(&library.favorites().unwrap()).unwrap();
        assert!(favorites_json.contains("Fixture"));
        assert!(!favorites_json.contains(&favorite_root.to_string_lossy().to_string()));
        let traversal = RemoteAddress {
            root_id: favorite_id.to_string(),
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

    #[test]
    fn live_library_refreshes_add_rename_order_and_rejects_removed_favorites() {
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
                .validate_remote_address(&RemoteAddress::file(second_id.to_string(), "page.jpg"))
                .is_ok()
        );

        writer
            .execute(
                "DELETE FROM favorites WHERE id = ?1",
                [first_id.as_bytes().as_slice()],
            )
            .unwrap();
        assert!(matches!(
            library.validate_remote_address(&RemoteAddress::file(first_id.to_string(), "page.jpg")),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn one_generation_observes_favorites_and_view_trim_but_ignores_unrelated_settings() {
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
