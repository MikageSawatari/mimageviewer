//! Process-wide caption display cache, deliberately outside viewer bundles.
//!
//! UI calls only inspect memory or send a small request. One lazy worker owns
//! each short read-only SELECT and file stat; it never holds a transaction
//! between requests. Dropping the channels retires the worker without a join.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use crate::ai_caption_db::{CaptionDb, CaptionRow, FileSignature};
use crate::app::book_cover::{BookCover, BookCoverEnv, BookKind, resolve_book_cover};

const MAX_ENTRIES: usize = 512;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CaptionDisplayValue {
    pub row: Option<CaptionRow>,
    pub current: Option<CurrentCaptionSource>,
    pub rotation: crate::rotation_db::Rotation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CurrentCaptionSource {
    Image(FileSignature),
    Book {
        signature: FileSignature,
        cover_ref: String,
    },
    VideoFrame {
        signature: FileSignature,
        cover_ref: String,
    },
}

impl CurrentCaptionSource {
    fn differs_from(&self, row: &CaptionRow, rotation: crate::rotation_db::Rotation) -> bool {
        match self {
            Self::Image(signature) => !row.matches_source(*signature, "", rotation),
            Self::Book {
                signature,
                cover_ref,
            }
            | Self::VideoFrame {
                signature,
                cover_ref,
            } => !row.matches_source(*signature, cover_ref, rotation),
        }
    }

    #[cfg(test)]
    pub(crate) fn signature_mut(&mut self) -> &mut FileSignature {
        match self {
            Self::Image(signature)
            | Self::Book { signature, .. }
            | Self::VideoFrame { signature, .. } => signature,
        }
    }
}

impl CaptionDisplayValue {
    /// Missing files belong to the list's lifecycle, not the caption warning.
    pub fn is_stale(&self) -> bool {
        self.row
            .as_ref()
            .zip(self.current.as_ref())
            .is_some_and(|(row, current)| current.differs_from(row, self.rotation))
    }

    pub fn is_source_stale(&self) -> bool {
        self.row
            .as_ref()
            .zip(self.current.as_ref())
            .is_some_and(|(row, current)| current.differs_from(row, row.rotation))
    }

    pub fn has_video_cover(&self) -> bool {
        matches!(self.current, Some(CurrentCaptionSource::VideoFrame { .. }))
    }
}

enum DisplaySource {
    Image { books_root: Option<PathBuf> },
    Book { kind: BookKind, env: BookCoverEnv },
}

enum Entry {
    Pending {
        previous: Option<CaptionDisplayValue>,
        generation: u64,
        changed_rotation_keys: Vec<Arc<HashSet<String>>>,
    },
    Loaded {
        value: CaptionDisplayValue,
        generation: u64,
        rotation_key: Option<String>,
    },
}

impl Entry {
    fn generation(&self) -> u64 {
        match self {
            Self::Pending { generation, .. } | Self::Loaded { generation, .. } => *generation,
        }
    }

    fn value(&self) -> Option<&CaptionDisplayValue> {
        match self {
            Self::Loaded { value, .. } => Some(value),
            Self::Pending { previous, .. } => previous.as_ref(),
        }
    }
}

struct ReadRequest {
    key: String,
    path: PathBuf,
    source: DisplaySource,
    epoch: u64,
    generation: u64,
    ctx: egui::Context,
}

#[derive(Debug)]
struct ReadResult {
    key: String,
    epoch: u64,
    generation: u64,
    value: CaptionDisplayValue,
    rotation_key: Option<String>,
}

struct Worker {
    requests: Sender<ReadRequest>,
    results: Receiver<ReadResult>,
}

impl Worker {
    fn start(db_path: Option<PathBuf>) -> std::io::Result<Self> {
        let (requests, request_rx) = mpsc::channel::<ReadRequest>();
        let (result_tx, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("caption-display".into())
            .spawn(move || {
                // Resolve the application data path on the worker as well.
                let db_path = db_path.unwrap_or_else(CaptionDb::db_path);
                while let Ok(request) = request_rx.recv() {
                    let (value, rotation_key) = read_value_for_source(
                        &db_path,
                        &request.key,
                        &request.path,
                        &request.source,
                    );
                    let result = ReadResult {
                        key: request.key,
                        epoch: request.epoch,
                        generation: request.generation,
                        value,
                        rotation_key,
                    };
                    if result_tx.send(result).is_err() {
                        break;
                    }
                    // Only the root drains this shared cache, even when the
                    // request was made while drawing a detached viewport.
                    request.ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            })?;
        Ok(Self { requests, results })
    }
}

#[cfg(test)]
fn read_value(db_path: &Path, key: &str, path: &Path) -> CaptionDisplayValue {
    read_value_for_source(
        db_path,
        key,
        path,
        &DisplaySource::Image { books_root: None },
    )
    .0
}

fn read_value_for_source(
    db_path: &Path,
    key: &str,
    path: &Path,
    source: &DisplaySource,
) -> (CaptionDisplayValue, Option<String>) {
    let started = Instant::now();
    // Open anew for each request so a store first created by a job, or replaced
    // by an import, is observed. get() finishes its statement before stat().
    let read = CaptionDb::open_read_only(db_path).and_then(|db| match db {
        Some(db) => db.get(key),
        None => Ok(None),
    });
    let (row, read_failed) = match read {
        Ok(row) => (row, false),
        Err(error) => {
            crate::logger::log(format!("caption display: reading {key} failed: {error}"));
            (None, true)
        }
    };
    let mut rotation_key = match source {
        DisplaySource::Image { books_root } => crate::edit_source::caption_rotation_key(
            &crate::thumb_loader::ReadLocation::File(path.to_owned()),
            books_root.as_deref(),
        ),
        DisplaySource::Book { .. } => None,
    };
    let current = match source {
        DisplaySource::Image { .. } => std::fs::metadata(path)
            .ok()
            .as_ref()
            .and_then(FileSignature::from_metadata)
            .map(CurrentCaptionSource::Image),
        // No saved row means there is nothing to compare. In particular, merely
        // viewing an uncatalogued folder does not start a representative scan.
        DisplaySource::Book { kind, env } if row.is_some() => {
            match resolve_book_cover(path, *kind, env) {
                BookCover::Readable(cover) => {
                    // Resolution already used the worker's effective bookshelf root.
                    rotation_key = cover.rotation_key;
                    Some(CurrentCaptionSource::Book {
                        signature: cover.signature,
                        cover_ref: cover.cover_ref,
                    })
                }
                BookCover::VideoFrame {
                    signature,
                    cover_ref,
                    ..
                } => Some(CurrentCaptionSource::VideoFrame {
                    signature,
                    cover_ref,
                }),
                BookCover::Unreadable(_) => None,
            }
        }
        DisplaySource::Book { .. } => None,
    };
    let rotation_path = db_path.with_file_name("rotation.db");
    let rotation = (|| {
        if rotation_key.is_none()
            || !rotation_path
                .try_exists()
                .map_err(|_| rusqlite::Error::InvalidQuery)?
        {
            return Ok(crate::rotation_db::Rotation::None);
        }
        let db = crate::rotation_db::RotationDb::open_readonly_at(&rotation_path)?;
        crate::edit_source::read_caption_rotation(Some(&db), rotation_key.as_deref())
    })()
    .unwrap_or_else(|error: rusqlite::Error| {
        crate::logger::log(format!(
            "caption display: reading rotation for {key} failed: {error}"
        ));
        crate::rotation_db::Rotation::None
    });
    if crate::perf::is_enabled() {
        crate::perf::event(
            "ai",
            "caption_display_read",
            Some(key),
            0,
            &[
                (
                    "ms",
                    serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                ),
                ("has_row", serde_json::json!(row.is_some())),
                ("has_signature", serde_json::json!(current.is_some())),
                ("failed", serde_json::json!(read_failed)),
            ],
        );
    }
    (
        CaptionDisplayValue {
            row,
            current,
            rotation,
        },
        rotation_key,
    )
}

#[derive(Default)]
pub(crate) struct CaptionDisplayCache {
    entries: HashMap<String, Entry>,
    epoch: u64,
    // Fresh request generations replace per-key tombstones: invalidating a
    // key and requesting it again cannot admit its old result, and the memory
    // limit applies to the whole cache rather than just its loaded rows.
    next_generation: u64,
    worker: Option<Worker>,
}

impl CaptionDisplayCache {
    pub fn get(&self, key: &str) -> Option<&CaptionDisplayValue> {
        self.entries.get(key).and_then(Entry::value)
    }

    /// A panel requests once on a miss, and passes refresh=true when it starts
    /// showing a key again, including while an older read is still in flight.
    /// Continuous drawing never resends a present key. Reentry supersedes its
    /// generation without an additional pending-refresh state.
    /// An existing value stays visible until its refresh result arrives.
    /// Reader startup/send failures return an error for the caller's feedback
    /// and cache an empty terminal value, avoiding another attempt every frame.
    pub fn request(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        path: &Path,
        refresh: bool,
    ) -> Result<bool, String> {
        self.request_source(
            ctx,
            key,
            path,
            refresh,
            DisplaySource::Image { books_root: None },
        )
    }

    pub fn request_image(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        path: &Path,
        refresh: bool,
        books_root: PathBuf,
    ) -> Result<bool, String> {
        self.request_source(
            ctx,
            key,
            path,
            refresh,
            DisplaySource::Image {
                books_root: Some(books_root),
            },
        )
    }

    /// UI callers check demand before capturing the password/settings snapshot.
    pub fn needs_request(&self, key: &str, refresh: bool) -> bool {
        refresh || !self.entries.contains_key(key)
    }

    pub fn request_book(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        path: &Path,
        refresh: bool,
        kind: BookKind,
        env: BookCoverEnv,
    ) -> Result<bool, String> {
        self.request_source(ctx, key, path, refresh, DisplaySource::Book { kind, env })
    }

    fn request_source(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        path: &Path,
        refresh: bool,
        source: DisplaySource,
    ) -> Result<bool, String> {
        if self.entries.contains_key(key) {
            if !refresh {
                return Ok(false);
            }
        } else if self.entries.len() >= MAX_ENTRIES {
            self.clear();
        }
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        let previous = self.entries.remove(key).and_then(|entry| match entry {
            Entry::Loaded { value, .. } => Some(value),
            Entry::Pending { previous, .. } => previous,
        });
        let entry = Entry::Pending {
            previous,
            generation,
            changed_rotation_keys: Vec::new(),
        };
        self.entries.insert(key.to_owned(), entry);
        let request = ReadRequest {
            key: key.to_owned(),
            path: path.to_owned(),
            source,
            epoch: self.epoch,
            generation,
            ctx: ctx.clone(),
        };
        if self.worker.is_none() {
            match Worker::start(None) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    crate::logger::log(format!("caption display: starting reader failed: {error}"));
                    self.adopt(ReadResult {
                        key: request.key,
                        epoch: request.epoch,
                        generation,
                        value: CaptionDisplayValue::default(),
                        rotation_key: None,
                    });
                    return Err(error.to_string());
                }
            }
        }
        if let Err(error) = self
            .worker
            .as_ref()
            .expect("started reader")
            .requests
            .send(request)
        {
            crate::logger::log("caption display: reader disconnected");
            let request = error.0;
            self.adopt(ReadResult {
                key: request.key,
                epoch: request.epoch,
                generation,
                value: CaptionDisplayValue::default(),
                rotation_key: None,
            });
            return Err("caption display reader disconnected".into());
        }
        Ok(true)
    }

    pub fn invalidate(&mut self, key: &str) {
        self.entries.remove(key);
    }

    /// Pending books resolve their dependency on the worker. Remember changes
    /// with that request, then reject only a result which actually used a changed key.
    pub(crate) fn invalidate_rotation_keys<'a>(&mut self, keys: impl IntoIterator<Item = &'a str>) {
        let keys: Arc<HashSet<String>> = Arc::new(keys.into_iter().map(str::to_owned).collect());
        if keys.is_empty() {
            return;
        }
        self.entries.retain(|_, entry| match entry {
            Entry::Loaded { rotation_key, .. } => {
                !rotation_key.as_ref().is_some_and(|key| keys.contains(key))
            }
            Entry::Pending {
                changed_rotation_keys,
                ..
            } => {
                changed_rotation_keys.push(Arc::clone(&keys));
                true
            }
        });
    }

    pub fn clear(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.entries.clear();
    }

    /// Nonblocking root-frame receive. Returns whether any current value changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            let result = self
                .worker
                .as_ref()
                .and_then(|worker| worker.results.try_recv().ok());
            let Some(result) = result else { break };
            changed |= self.adopt(result);
        }
        changed
    }

    fn adopt(&mut self, result: ReadResult) -> bool {
        if result.epoch != self.epoch
            || self.entries.get(&result.key).map(Entry::generation) != Some(result.generation)
        {
            return false;
        }
        if let Some(Entry::Pending {
            changed_rotation_keys,
            ..
        }) = self.entries.get(&result.key)
            && result
                .rotation_key
                .as_ref()
                .is_some_and(|key| changed_rotation_keys.iter().any(|keys| keys.contains(key)))
        {
            self.entries.remove(&result.key);
            return true;
        }
        self.entries.insert(
            result.key,
            Entry::Loaded {
                value: result.value,
                generation: result.generation,
                rotation_key: result.rotation_key,
            },
        );
        true
    }

    #[cfg(test)]
    pub(crate) fn seed_for_test(&mut self, key: &str) {
        self.next_generation = self.next_generation.wrapping_add(1);
        self.entries.insert(
            key.to_owned(),
            Entry::Pending {
                previous: None,
                generation: self.next_generation,
                changed_rotation_keys: Vec::new(),
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn contains_for_test(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    #[cfg(test)]
    pub(crate) fn seed_rotation_for_test(&mut self, key: &str, rotation_key: &str) {
        self.next_generation = self.next_generation.wrapping_add(1);
        self.entries.insert(
            key.to_owned(),
            Entry::Loaded {
                value: CaptionDisplayValue::default(),
                generation: self.next_generation,
                rotation_key: Some(rotation_key.to_owned()),
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn epoch_for_test(&self) -> u64 {
        self.epoch
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_caption_db::{CaptionResult, CaptionStatus, SaveRequest};
    use std::time::Duration;

    fn fake_cache() -> (
        CaptionDisplayCache,
        Receiver<ReadRequest>,
        Sender<ReadResult>,
    ) {
        let (requests, request_rx) = mpsc::channel();
        let (result_tx, results) = mpsc::channel();
        let cache = CaptionDisplayCache {
            worker: Some(Worker { requests, results }),
            ..Default::default()
        };
        (cache, request_rx, result_tx)
    }

    fn row(signature: FileSignature) -> CaptionRow {
        CaptionRow {
            rotation: crate::rotation_db::Rotation::None,
            item_key: "image".into(),
            signature,
            model_id: "model@revision".into(),
            prompt_version: 1,
            lang: "ja".into(),
            caption: "青い空".into(),
            visible_text: String::new(),
            keywords: Vec::new(),
            cover_ref: String::new(),
            status: CaptionStatus::Ok,
            created_at: 100,
            rev: 1,
            indexed: false,
        }
    }

    fn value() -> CaptionDisplayValue {
        let signature = FileSignature {
            size: 5,
            mtime_ms: 10,
        };
        CaptionDisplayValue {
            rotation: crate::rotation_db::Rotation::None,
            row: Some(row(signature)),
            current: Some(crate::caption_display_cache::CurrentCaptionSource::Image(
                signature,
            )),
        }
    }

    fn result(request: ReadRequest, value: CaptionDisplayValue) -> ReadResult {
        ReadResult {
            key: request.key,
            epoch: request.epoch,
            generation: request.generation,
            value,
            rotation_key: Some(crate::adjustment_db::normalize_path(&request.path)),
        }
    }

    #[test]
    fn caption_display_deduplicates_requests_and_retains_loaded_value_during_refresh() {
        let (mut cache, requests, results) = fake_cache();
        let ctx = egui::Context::default();
        let path = Path::new("image");
        assert!(cache.request(&ctx, "image", path, false).unwrap());
        assert!(!cache.request(&ctx, "image", path, false).unwrap());
        assert!(!cache.request(&ctx, "image", path, false).unwrap());
        assert!(cache.get("image").is_none());
        let initial = requests.try_recv().unwrap();
        assert!(requests.try_recv().is_err());
        results.send(result(initial, value())).unwrap();
        assert!(cache.poll());
        assert_eq!(cache.get("image"), Some(&value()));
        assert!(!cache.request(&ctx, "image", path, false).unwrap());

        assert!(cache.request(&ctx, "image", path, true).unwrap());
        assert_eq!(cache.get("image"), Some(&value()));
        assert!(!cache.request(&ctx, "image", path, false).unwrap());
        let refresh = requests.try_recv().unwrap();
        assert!(requests.try_recv().is_err());
        results
            .send(result(refresh, CaptionDisplayValue::default()))
            .unwrap();
        assert!(cache.poll());
        assert_eq!(cache.get("image"), Some(&CaptionDisplayValue::default()));
        assert!(!cache.poll());
    }

    #[test]
    fn caption_display_reentry_supersedes_loading_and_reads_overwritten_signature() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("ai_caption.db");
        let path = temp.path().join("a.jpg");
        let other = temp.path().join("b.jpg");
        std::fs::write(&path, b"original image").unwrap();
        let signature = FileSignature::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
        let key = CaptionDb::normalize_path(&path);
        CaptionDb::open_at(&db_path)
            .unwrap()
            .save(&SaveRequest {
                rotation: crate::rotation_db::Rotation::None,
                item_path: path.clone(),
                signature_path: path.clone(),
                signature,
                cover_ref: String::new(),
                model_id: "fixture".into(),
                prompt_version: 1,
                lang: "ja".into(),
                result: CaptionResult::Ok {
                    caption: "saved description".into(),
                    text: String::new(),
                    keywords: Vec::new(),
                },
            })
            .unwrap();
        let (mut cache, requests, results) = fake_cache();
        let ctx = egui::Context::default();
        cache.request(&ctx, &key, &path, true).unwrap();
        let old = requests.try_recv().unwrap();
        // The first read has finished stat, but its completion has not reached root poll.
        let old_value = read_value(&db_path, &old.key, &old.path);
        assert!(!old_value.is_stale());
        cache.request(&ctx, "other", &other, true).unwrap();
        let _other = requests.try_recv().unwrap();
        std::fs::write(&path, b"externally overwritten, longer image").unwrap();
        assert!(cache.request(&ctx, &key, &path, true).unwrap());
        let newest = requests.try_recv().unwrap();
        assert_ne!(old.generation, newest.generation);
        assert!(!cache.request(&ctx, &key, &path, false).unwrap());
        assert!(requests.try_recv().is_err());
        results.send(result(old, old_value)).unwrap();
        assert!(!cache.poll());
        assert!(cache.get(&key).is_none());
        let newest_value = read_value(&db_path, &newest.key, &newest.path);
        assert!(newest_value.is_stale());
        results.send(result(newest, newest_value)).unwrap();
        assert!(cache.poll());
        assert!(cache.get(&key).unwrap().is_stale());
        assert_eq!(
            cache.get(&key).unwrap().row.as_ref().unwrap().caption,
            "saved description"
        );
    }

    #[test]
    fn caption_display_reentry_during_loaded_refresh_preserves_value_and_latest_generation() {
        let (mut cache, requests, results) = fake_cache();
        let ctx = egui::Context::default();
        let path = Path::new("image");
        cache.request(&ctx, "image", path, true).unwrap();
        results
            .send(result(requests.try_recv().unwrap(), value()))
            .unwrap();
        cache.poll();
        cache.request(&ctx, "image", path, true).unwrap();
        let old = requests.try_recv().unwrap();
        cache.request(&ctx, "image", path, true).unwrap();
        let newest = requests.try_recv().unwrap();
        assert_eq!(cache.get("image"), Some(&value()));
        results
            .send(result(old, CaptionDisplayValue::default()))
            .unwrap();
        assert!(!cache.poll());
        assert_eq!(cache.get("image"), Some(&value()));
        results
            .send(result(newest, CaptionDisplayValue::default()))
            .unwrap();
        assert!(cache.poll());
        assert_eq!(cache.get("image"), Some(&CaptionDisplayValue::default()));
    }

    #[test]
    fn caption_display_disconnected_reader_returns_error_and_finishes_the_request() {
        let (mut cache, requests, _results) = fake_cache();
        drop(requests);
        let ctx = egui::Context::default();
        assert!(
            cache
                .request(&ctx, "image", Path::new("image"), false)
                .is_err()
        );
        assert_eq!(cache.get("image"), Some(&CaptionDisplayValue::default()));
        assert!(
            !cache
                .request(&ctx, "image", Path::new("image"), false)
                .unwrap()
        );
    }

    #[test]
    fn caption_display_key_invalidation_rejects_old_generation_and_preserves_siblings() {
        let (mut cache, requests, results) = fake_cache();
        let ctx = egui::Context::default();
        let path = Path::new("image");
        cache.request(&ctx, "image", path, false).unwrap();
        let old = requests.try_recv().unwrap();
        cache.request(&ctx, "sibling", path, false).unwrap();
        let sibling = requests.try_recv().unwrap();
        results.send(result(sibling, value())).unwrap();
        cache.poll();
        cache.invalidate("image");
        assert_eq!(cache.get("sibling"), Some(&value()));
        cache.request(&ctx, "image", path, false).unwrap();
        let new = requests.try_recv().unwrap();
        assert_ne!(old.generation, new.generation);
        results.send(result(old, value())).unwrap();
        assert!(!cache.poll());
        assert!(cache.get("image").is_none());
        results
            .send(result(new, CaptionDisplayValue::default()))
            .unwrap();
        assert!(cache.poll());
        assert_eq!(cache.get("image"), Some(&CaptionDisplayValue::default()));
        assert_eq!(cache.get("sibling"), Some(&value()));
    }

    #[test]
    fn caption_display_clear_rejects_in_flight_results_even_after_the_same_key_is_requested() {
        let (mut cache, requests, results) = fake_cache();
        let ctx = egui::Context::default();
        let path = Path::new("image");
        cache.request(&ctx, "image", path, false).unwrap();
        let old = requests.try_recv().unwrap();
        cache.clear();
        cache.request(&ctx, "image", path, false).unwrap();
        let new = requests.try_recv().unwrap();
        assert_ne!(old.epoch, new.epoch);
        results.send(result(old, value())).unwrap();
        assert!(!cache.poll());
        assert!(cache.get("image").is_none());
        results.send(result(new, value())).unwrap();
        assert!(cache.poll());
        assert_eq!(cache.get("image"), Some(&value()));
    }

    #[test]
    fn caption_display_capacity_clears_every_entry_and_invalidates_pending_reads() {
        let (mut cache, requests, results) = fake_cache();
        let ctx = egui::Context::default();
        let path = Path::new("image");
        for index in 0..MAX_ENTRIES {
            cache
                .request(&ctx, &index.to_string(), path, false)
                .unwrap();
        }
        let old = requests.try_recv().unwrap();
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        let epoch = cache.epoch;
        cache.request(&ctx, "overflow", path, false).unwrap();
        assert_ne!(cache.epoch, epoch);
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.entries.contains_key("overflow"));
        results.send(result(old, value())).unwrap();
        assert!(!cache.poll());
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn caption_display_stale_requires_a_row_and_an_available_different_signature() {
        let mut value = value();
        assert!(!value.is_stale());
        value.current.as_mut().unwrap().signature_mut().size += 1;
        assert!(value.is_stale());
        value.current = Some(CurrentCaptionSource::Image(
            value.row.as_ref().unwrap().signature,
        ));
        value.current.as_mut().unwrap().signature_mut().mtime_ms += 1;
        assert!(value.is_stale());
        value.current = None;
        assert!(!value.is_stale());
        value.row = None;
        value.current = Some(CurrentCaptionSource::Image(FileSignature {
            size: 1,
            mtime_ms: 2,
        }));
        assert!(!value.is_stale());
    }

    #[test]
    fn caption_rotation_invalidation_preserves_loaded_and_pending_siblings() {
        let (mut cache, requests, results) = fake_cache();
        let ctx = egui::Context::default();
        for key in ["image", "sibling", "pending-sibling", "book"] {
            cache.request(&ctx, key, Path::new(key), false).unwrap();
        }
        let image = requests.try_recv().unwrap();
        let sibling = requests.try_recv().unwrap();
        let pending_sibling = requests.try_recv().unwrap();
        let book = requests.try_recv().unwrap();
        results.send(result(image, value())).unwrap();
        results.send(result(sibling, value())).unwrap();
        cache.poll();
        cache.invalidate_rotation_keys(["image"]);
        assert!(cache.get("image").is_none());
        assert_eq!(cache.get("sibling"), Some(&value()));
        let mut book = result(book, value());
        // The worker resolves this dependency only after the request was sent.
        book.rotation_key = Some("image".into());
        results.send(book).unwrap();
        results.send(result(pending_sibling, value())).unwrap();
        cache.poll();
        assert!(!cache.contains_for_test("book"));
        assert_eq!(cache.get("pending-sibling"), Some(&value()));
        assert_eq!(cache.get("sibling"), Some(&value()));
        cache
            .request(&ctx, "image", Path::new("image"), false)
            .unwrap();
        let request = requests.try_recv().unwrap();
        let mut rotated = value();
        rotated.rotation = crate::rotation_db::Rotation::Cw90;
        results.send(result(request, rotated.clone())).unwrap();
        cache.poll();
        assert_eq!(cache.get("image"), Some(&rotated));
        assert!(rotated.is_stale());
        assert!(!rotated.is_source_stale());
    }

    #[test]
    fn caption_rotation_worker_rereads_angle_after_key_invalidation() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cover.png");
        std::fs::write(&path, b"cover").unwrap();
        let db_path = tmp.path().join("ai_caption.db");
        let db = CaptionDb::open_at(&db_path).unwrap();
        let signature = file_signature_for_test(&path);
        db.save(&SaveRequest {
            item_path: path.clone(),
            signature_path: path.clone(),
            signature,
            rotation: crate::rotation_db::Rotation::None,
            cover_ref: String::new(),
            model_id: "fixture".into(),
            prompt_version: 1,
            lang: "ja".into(),
            result: CaptionResult::Ok {
                caption: "cover".into(),
                text: String::new(),
                keywords: Vec::new(),
            },
        })
        .unwrap();
        let key = CaptionDb::normalize_path(&path);
        let rotation_key = crate::edit_source::page_key_for_grid_item(
            &crate::grid_item::GridItem::Image(path.clone()),
        )
        .unwrap();
        let mut cache = CaptionDisplayCache {
            worker: Some(Worker::start(Some(db_path)).unwrap()),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        cache.request(&ctx, &key, &path, false).unwrap();
        receive_worker(&mut cache);
        assert!(!cache.get(&key).unwrap().is_stale());
        let rotations =
            crate::rotation_db::RotationDb::open_at(&tmp.path().join("rotation.db")).unwrap();
        rotations
            .set_key(&rotation_key, crate::rotation_db::Rotation::Cw270)
            .unwrap();
        cache.invalidate_rotation_keys([rotation_key.as_str()]);
        cache.request(&ctx, &key, &path, false).unwrap();
        receive_worker(&mut cache);
        assert!(cache.get(&key).unwrap().is_stale());
        assert!(!cache.get(&key).unwrap().is_source_stale());
        assert_eq!(
            cache.get(&key).unwrap().rotation,
            crate::rotation_db::Rotation::Cw270
        );
    }

    fn receive_worker(cache: &mut CaptionDisplayCache) {
        let result = cache
            .worker
            .as_ref()
            .unwrap()
            .results
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(cache.adopt(result));
    }

    #[test]
    fn caption_display_worker_db_error_is_an_empty_cached_row_with_current_signature() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("ai_caption.db");
        std::fs::write(&db_path, b"not a SQLite database").unwrap();
        let path = temp.path().join("image.jpg");
        std::fs::write(&path, b"image").unwrap();
        let signature = FileSignature::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
        let ctx = egui::Context::default();
        let mut cache = CaptionDisplayCache {
            worker: Some(Worker::start(Some(db_path)).unwrap()),
            ..Default::default()
        };
        cache.request(&ctx, "image", &path, false).unwrap();
        receive_worker(&mut cache);
        assert_eq!(
            cache.get("image"),
            Some(&CaptionDisplayValue {
                rotation: crate::rotation_db::Rotation::None,
                row: None,
                current: Some(crate::caption_display_cache::CurrentCaptionSource::Image(
                    signature
                ))
            })
        );
        assert!(!cache.request(&ctx, "image", &path, false).unwrap());
    }

    #[test]
    fn caption_display_worker_rereads_file_signature_and_newly_created_or_updated_store() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("ai_caption.db");
        let path = temp.path().join("image.jpg");
        std::fs::write(&path, b"image").unwrap();
        let key = CaptionDb::normalize_path(&path);
        let signature = FileSignature::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
        let ctx = egui::Context::default();
        let mut cache = CaptionDisplayCache {
            worker: Some(Worker::start(Some(db_path.clone())).unwrap()),
            ..Default::default()
        };
        cache.request(&ctx, &key, &path, false).unwrap();
        receive_worker(&mut cache);
        assert_eq!(cache.get(&key).unwrap().row, None);
        assert!(!db_path.exists());
        let db = CaptionDb::open_at(&db_path).unwrap();
        let mut request = SaveRequest {
            rotation: crate::rotation_db::Rotation::None,
            item_path: path.clone(),
            signature_path: path.clone(),
            signature,
            cover_ref: String::new(),
            model_id: "model@revision".into(),
            prompt_version: 1,
            lang: "ja".into(),
            result: CaptionResult::Ok {
                caption: "初回".into(),
                text: String::new(),
                keywords: Vec::new(),
            },
        };
        db.save(&request).unwrap();
        cache.request(&ctx, &key, &path, true).unwrap();
        receive_worker(&mut cache);
        assert_eq!(
            cache.get(&key).unwrap().row.as_ref().unwrap().caption,
            "初回"
        );
        assert!(!cache.get(&key).unwrap().is_stale());

        // Size is deterministic even on filesystems with coarse mtime clocks.
        std::fs::write(&path, b"externally overwritten image").unwrap();
        cache.request(&ctx, &key, &path, true).unwrap();
        assert!(!cache.get(&key).unwrap().is_stale());
        receive_worker(&mut cache);
        assert!(cache.get(&key).unwrap().is_stale());
        assert_eq!(
            cache.get(&key).unwrap().row.as_ref().unwrap().caption,
            "初回"
        );

        request.signature =
            FileSignature::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
        request.result = CaptionResult::Ok {
            caption: "更新後".into(),
            text: String::new(),
            keywords: Vec::new(),
        };
        db.save(&request).unwrap();
        cache.invalidate(&key);
        cache.request(&ctx, &key, &path, false).unwrap();
        receive_worker(&mut cache);
        assert_eq!(
            cache.get(&key).unwrap().row.as_ref().unwrap().caption,
            "更新後"
        );
        assert!(!cache.get(&key).unwrap().is_stale());
        std::fs::remove_file(&path).unwrap();
        cache.request(&ctx, &key, &path, true).unwrap();
        receive_worker(&mut cache);
        assert!(!cache.get(&key).unwrap().is_stale());
        assert_eq!(
            cache.get(&key).unwrap().row.as_ref().unwrap().caption,
            "更新後"
        );
    }

    #[test]
    fn caption_book_display_worker_resolves_changed_pin_and_missing_book_without_rewriting_row() {
        use crate::folder_thumb_pins::{FileKind, FolderPinSource, FolderThumbPinDb};
        use std::sync::Arc;
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("book");
        std::fs::create_dir(&folder).unwrap();
        let first = folder.join("a.png");
        std::fs::write(&first, b"cover A").unwrap();
        std::fs::write(folder.join("b.png"), b"cover B").unwrap();
        let signature = FileSignature::from_metadata(&std::fs::metadata(&first).unwrap()).unwrap();
        let db_path = tmp.path().join("ai_caption.db");
        let db = CaptionDb::open_at(&db_path).unwrap();
        db.save(&SaveRequest {
            rotation: crate::rotation_db::Rotation::None,
            item_path: folder.clone(),
            signature_path: first,
            signature,
            cover_ref: "a.png".into(),
            model_id: "fixture".into(),
            prompt_version: 1,
            lang: "ja".into(),
            result: CaptionResult::Ok {
                caption: "saved book description".into(),
                text: String::new(),
                keywords: Vec::new(),
            },
        })
        .unwrap();
        let pins = Arc::new(FolderThumbPinDb::open_at(&tmp.path().join("pins.db")).unwrap());
        let source = |rel: &str| FolderPinSource::File {
            rel: rel.into(),
            kind: FileKind::Image,
        };
        pins.set(&folder, &source("a.png")).unwrap();
        let env = BookCoverEnv {
            pin_db: Some(Arc::clone(&pins)),
            ..Default::default()
        };
        let key = CaptionDb::normalize_path(&folder);
        let ctx = egui::Context::default();
        let mut cache = CaptionDisplayCache {
            worker: Some(Worker::start(Some(db_path)).unwrap()),
            ..Default::default()
        };
        cache
            .request_book(&ctx, &key, &folder, false, BookKind::Folder, env.clone())
            .unwrap();
        receive_worker(&mut cache);
        assert!(!cache.get(&key).unwrap().is_stale());
        assert!(
            !cache
                .request_book(&ctx, &key, &folder, false, BookKind::Folder, env.clone())
                .unwrap()
        );
        let original = db.get(&key).unwrap().unwrap();
        pins.set(&folder, &source("b.png")).unwrap();
        cache.clear();
        cache
            .request_book(&ctx, &key, &folder, false, BookKind::Folder, env.clone())
            .unwrap();
        receive_worker(&mut cache);
        let value = cache.get(&key).unwrap();
        assert!(value.is_stale());
        assert!(
            matches!(&value.current, Some(CurrentCaptionSource::Book { cover_ref, .. }) if cover_ref == "b.png")
        );
        assert_eq!(db.get(&key).unwrap(), Some(original.clone()));
        std::fs::rename(&folder, tmp.path().join("removed-book")).unwrap();
        cache
            .request_book(&ctx, &key, &folder, true, BookKind::Folder, env)
            .unwrap();
        receive_worker(&mut cache);
        let value = cache.get(&key).unwrap();
        assert!(value.current.is_none());
        assert!(!value.is_stale());
        assert_eq!(value.row.as_ref(), Some(&original));
    }

    #[test]
    fn caption_book_display_cover_identity_changes_even_when_file_signature_does_not() {
        let mut value = value();
        let row = value.row.as_mut().unwrap();
        row.cover_ref = "#page=0".into();
        value.current = Some(CurrentCaptionSource::Book {
            signature: row.signature,
            cover_ref: "#page=2".into(),
        });
        assert!(value.is_stale());
        assert!(!value.has_video_cover());
        value.current = Some(CurrentCaptionSource::VideoFrame {
            signature: value.row.as_ref().unwrap().signature,
            cover_ref: "clip.mp4#frame=3".into(),
        });
        assert!(value.is_stale());
        assert!(value.has_video_cover());
        value.current = None;
        assert!(!value.is_stale());
    }

    #[test]
    fn caption_book_display_adopted_extension_named_folder_matches_creation_and_pin_changes() {
        use crate::app::book_cover::BookCover;
        use crate::app::book_materialization::{BookScope, ProjectionRecipe};
        use crate::folder_thumb_pins::{FileKind, FolderPinSource};
        use crate::grid_item::GridItem;
        let mut app = crate::app::setup_app_for_test();
        let tmp = tempfile::tempdir().unwrap();
        for (name, expected_hint) in [("album.pdf", BookKind::Pdf), ("album.zip", BookKind::Zip)] {
            let folder = tmp.path().join(name);
            std::fs::create_dir(&folder).unwrap();
            let first = folder.join("a.png");
            std::fs::write(&first, b"first cover").unwrap();
            std::fs::write(folder.join("b.png"), b"second cover").unwrap();
            app.current_folder = Some(folder.clone());
            app.items = vec![GridItem::Image(first.clone())];
            app.fullscreen_idx = Some(0);
            // Use the real accepted recipe, which classifies these directories
            // by extension. The worker must correct the hint without UI stat.
            app.projection_recipe =
                ProjectionRecipe::for_physical(&app.settings, &folder, &folder, &app.items);
            assert!(matches!(
                &app.projection_recipe,
                ProjectionRecipe::Book(BookScope::Document(_))
                    | ProjectionRecipe::Book(BookScope::Archive { .. })
            ));
            let (key, path, hint) = app.current_caption_book_target().unwrap();
            assert_eq!(hint, expected_hint);
            assert_eq!(key, CaptionDb::normalize_path(&folder));
            assert_eq!(path, folder);
            let source = |rel: &str| FolderPinSource::File {
                rel: rel.into(),
                kind: FileKind::Image,
            };
            assert!(app.set_folder_thumb_pin(&folder, source("a.png")));
            let env = app.book_cover_env();
            let BookCover::Readable(created) = resolve_book_cover(&folder, BookKind::Folder, &env)
            else {
                panic!("selected folder cover must be readable");
            };
            assert_eq!(created.cover_ref, "a.png");
            assert_eq!(created.signature_path, first);
            let db_path = tmp.path().join(format!("{name}.captions.db"));
            let db = CaptionDb::open_at(&db_path).unwrap();
            assert_eq!(
                db.save(&SaveRequest {
                    rotation: crate::rotation_db::Rotation::None,
                    item_path: folder.clone(),
                    signature_path: created.signature_path.clone(),
                    signature: created.signature,
                    cover_ref: created.cover_ref.clone(),
                    model_id: "fixture".into(),
                    prompt_version: 1,
                    lang: "ja".into(),
                    result: CaptionResult::Ok {
                        caption: "saved cover".into(),
                        text: String::new(),
                        keywords: Vec::new()
                    },
                })
                .unwrap(),
                crate::ai_caption_db::SaveOutcome::Written
            );
            app.caption_display = CaptionDisplayCache {
                worker: Some(Worker::start(Some(db_path)).unwrap()),
                ..Default::default()
            };
            let ctx = egui::Context::default();
            app.caption_display
                .request_book(&ctx, &key, &path, false, hint, env)
                .unwrap();
            receive_worker(&mut app.caption_display);
            let shown = app.caption_display.get(&key).unwrap();
            assert!(!shown.is_stale());
            assert_eq!(
                shown.current,
                Some(CurrentCaptionSource::Book {
                    signature: created.signature,
                    cover_ref: created.cover_ref
                })
            );
            assert!(app.set_folder_thumb_pin(&folder, source("b.png")));
            assert!(!app.caption_display.contains_for_test(&key));
            let env = app.book_cover_env();
            app.caption_display
                .request_book(&ctx, &key, &path, false, hint, env)
                .unwrap();
            receive_worker(&mut app.caption_display);
            let shown = app.caption_display.get(&key).unwrap();
            assert!(shown.is_stale());
            assert!(
                matches!(&shown.current, Some(CurrentCaptionSource::Book { cover_ref, signature })
                if cover_ref == "b.png" && *signature == file_signature_for_test(&folder.join("b.png")))
            );
        }
    }

    fn file_signature_for_test(path: &Path) -> FileSignature {
        FileSignature::from_metadata(&std::fs::metadata(path).unwrap()).unwrap()
    }

    #[test]
    fn caption_book_display_invalidation_discards_pending_old_cover_result() {
        let (mut cache, requests, results) = fake_cache();
        let ctx = egui::Context::default();
        let path = Path::new("book.zip");
        cache
            .request_book(
                &ctx,
                "book",
                path,
                false,
                BookKind::Zip,
                BookCoverEnv::default(),
            )
            .unwrap();
        let old = requests.try_recv().unwrap();
        cache.clear();
        cache
            .request_book(
                &ctx,
                "book",
                path,
                false,
                BookKind::Zip,
                BookCoverEnv::default(),
            )
            .unwrap();
        let new = requests.try_recv().unwrap();
        assert!(matches!(
            new.source,
            DisplaySource::Book {
                kind: BookKind::Zip,
                ..
            }
        ));
        results.send(result(old, value())).unwrap();
        assert!(!cache.poll());
        results
            .send(result(new, CaptionDisplayValue::default()))
            .unwrap();
        assert!(cache.poll());
        assert_eq!(cache.get("book"), Some(&CaptionDisplayValue::default()));
    }
}
