//! Process-wide caption display cache, deliberately outside viewer bundles.
//!
//! UI calls only inspect memory or send a small request. One lazy worker owns
//! each short read-only SELECT and file stat; it never holds a transaction
//! between requests. Dropping the channels retires the worker without a join.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use crate::ai_caption_db::{CaptionDb, CaptionRow, FileSignature};

const MAX_ENTRIES: usize = 512;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CaptionDisplayValue {
    pub row: Option<CaptionRow>,
    pub current: Option<FileSignature>,
}

impl CaptionDisplayValue {
    /// Missing files belong to the list's lifecycle, not the caption warning.
    pub fn is_stale(&self) -> bool {
        self.row
            .as_ref()
            .zip(self.current)
            .is_some_and(|(row, current)| row.signature != current)
    }
}

enum Entry {
    Loading {
        generation: u64,
    },
    Loaded {
        value: CaptionDisplayValue,
        generation: u64,
    },
}

impl Entry {
    fn generation(&self) -> u64 {
        match self {
            Self::Loading { generation } | Self::Loaded { generation, .. } => *generation,
        }
    }

    fn value(&self) -> Option<&CaptionDisplayValue> {
        match self {
            Self::Loaded { value, .. } => Some(value),
            Self::Loading { .. } => None,
        }
    }
}

struct ReadRequest {
    key: String,
    path: PathBuf,
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
                    let value = read_value(&db_path, &request.key, &request.path);
                    let result = ReadResult {
                        key: request.key,
                        epoch: request.epoch,
                        generation: request.generation,
                        value,
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

fn read_value(db_path: &Path, key: &str, path: &Path) -> CaptionDisplayValue {
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
    let current = std::fs::metadata(path)
        .ok()
        .as_ref()
        .and_then(FileSignature::from_metadata);
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
    CaptionDisplayValue { row, current }
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
        if self.entries.contains_key(key) {
            if !refresh {
                return Ok(false);
            }
        } else if self.entries.len() >= MAX_ENTRIES {
            self.clear();
        }
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        let entry = match self.entries.remove(key) {
            Some(Entry::Loaded {
                value,
                generation: previous_gen,
            }) => {
                debug_assert_ne!(generation, previous_gen);
                Entry::Loaded { value, generation }
            }
            _ => Entry::Loading { generation },
        };
        self.entries.insert(key.to_owned(), entry);
        let request = ReadRequest {
            key: key.to_owned(),
            path: path.to_owned(),
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
            });
            return Err("caption display reader disconnected".into());
        }
        Ok(true)
    }

    pub fn invalidate(&mut self, key: &str) {
        self.entries.remove(key);
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
        self.entries.insert(
            result.key,
            Entry::Loaded {
                value: result.value,
                generation: result.generation,
            },
        );
        true
    }

    #[cfg(test)]
    pub(crate) fn seed_for_test(&mut self, key: &str) {
        self.next_generation = self.next_generation.wrapping_add(1);
        self.entries.insert(
            key.to_owned(),
            Entry::Loading {
                generation: self.next_generation,
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn contains_for_test(&self, key: &str) -> bool {
        self.entries.contains_key(key)
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
            row: Some(row(signature)),
            current: Some(signature),
        }
    }

    fn result(request: ReadRequest, value: CaptionDisplayValue) -> ReadResult {
        ReadResult {
            key: request.key,
            epoch: request.epoch,
            generation: request.generation,
            value,
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
        value.current.as_mut().unwrap().size += 1;
        assert!(value.is_stale());
        value.current = Some(value.row.as_ref().unwrap().signature);
        value.current.as_mut().unwrap().mtime_ms += 1;
        assert!(value.is_stale());
        value.current = None;
        assert!(!value.is_stale());
        value.row = None;
        value.current = Some(FileSignature {
            size: 1,
            mtime_ms: 2,
        });
        assert!(!value.is_stale());
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
                row: None,
                current: Some(signature)
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
}
