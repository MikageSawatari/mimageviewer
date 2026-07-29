use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Instant;

use image::GenericImageView;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::diagnostics::{duration_ms, resolve_external_file_path};
use crate::image_support;

pub const GENERATED_THUMB_SIZE: u32 = 512;
const THUMB_WEBP_QUALITY: f32 = 78.0;

#[derive(Clone, Debug, Serialize)]
pub struct GenerationMetrics {
    pub source_width: u32,
    pub source_height: u32,
    pub output_width: u32,
    pub output_height: u32,
    pub decode_ms: f64,
    pub resize_ms: f64,
    pub webp_encode_ms: f64,
    pub total_ms: f64,
}

#[derive(Clone, Debug)]
pub enum ThumbnailServiceError {
    Decode,
    Db(String),
    Synchronization,
}

pub struct ThumbnailFetch {
    pub bytes: Arc<Vec<u8>>,
    pub remote_cache_hit: bool,
    pub waited_for_generation: bool,
    pub generation: Option<GenerationMetrics>,
}

struct GeneratedThumbnail {
    bytes: Arc<Vec<u8>>,
    metrics: GenerationMetrics,
}

type GenerateFn =
    dyn Fn(&Path, u32) -> Result<GeneratedThumbnail, ThumbnailServiceError> + Send + Sync;

pub struct ThumbnailService {
    cache: ThumbnailCache,
    inflight: Mutex<HashMap<String, Weak<Flight>>>,
    gate: GenerationGate,
    generator: Arc<GenerateFn>,
}

impl ThumbnailService {
    pub fn open(
        path: &Path,
        protected_roots: &[PathBuf],
        parallelism: usize,
    ) -> Result<Self, String> {
        Self::open_with_generator(
            path,
            protected_roots,
            parallelism,
            Arc::new(generate_thumbnail),
        )
    }

    fn open_with_generator(
        path: &Path,
        protected_roots: &[PathBuf],
        parallelism: usize,
        generator: Arc<GenerateFn>,
    ) -> Result<Self, String> {
        Ok(Self {
            cache: ThumbnailCache::open(path, protected_roots)?,
            inflight: Mutex::new(HashMap::new()),
            gate: GenerationGate::new(parallelism.clamp(1, 4)),
            generator,
        })
    }

    pub fn path(&self) -> &Path {
        &self.cache.path
    }

    pub fn parallelism(&self) -> usize {
        self.gate.limit
    }

    pub fn load_or_generate(
        &self,
        cache_key: &str,
        source_path: &Path,
        source_mtime_ns: u128,
        source_size: u64,
        generation_size: u32,
    ) -> Result<ThumbnailFetch, ThumbnailServiceError> {
        if let Some(bytes) = self.cache.load(cache_key)? {
            return Ok(ThumbnailFetch {
                bytes,
                remote_cache_hit: true,
                waited_for_generation: false,
                generation: None,
            });
        }

        let (flight, creator) = {
            let mut inflight = self
                .inflight
                .lock()
                .map_err(|_| ThumbnailServiceError::Synchronization)?;
            if inflight.len() > 1024 {
                inflight.retain(|_, flight| flight.strong_count() > 0);
            }
            if let Some(flight) = inflight.get(cache_key).and_then(Weak::upgrade) {
                (flight, false)
            } else {
                // A completed flight saves the SQLite row before releasing its
                // final strong reference. Recheck under the flight-map lock so
                // a request that raced with completion cannot start a duplicate.
                if let Some(bytes) = self.cache.load(cache_key)? {
                    return Ok(ThumbnailFetch {
                        bytes,
                        remote_cache_hit: true,
                        waited_for_generation: false,
                        generation: None,
                    });
                }
                let flight = Arc::new(Flight::new());
                inflight.insert(cache_key.to_owned(), Arc::downgrade(&flight));
                (flight, true)
            }
        };

        if !creator {
            let generated = flight.wait()?;
            return Ok(ThumbnailFetch {
                bytes: Arc::clone(&generated.bytes),
                remote_cache_hit: false,
                waited_for_generation: true,
                generation: Some(generated.metrics.clone()),
            });
        }

        let result = (|| {
            let _permit = self.gate.enter()?;
            let generated = (self.generator)(source_path, generation_size)?;
            self.cache.store(
                cache_key,
                source_mtime_ns,
                source_size,
                generation_size,
                generated.bytes.as_slice(),
            )?;
            Ok(Arc::new(generated))
        })();
        flight.complete(result.clone());
        let generated = result?;
        Ok(ThumbnailFetch {
            bytes: Arc::clone(&generated.bytes),
            remote_cache_hit: false,
            waited_for_generation: false,
            generation: Some(generated.metrics.clone()),
        })
    }
}

struct ThumbnailCache {
    path: PathBuf,
    connection: Mutex<Connection>,
}

impl ThumbnailCache {
    fn open(path: &Path, protected_roots: &[PathBuf]) -> Result<Self, String> {
        let path =
            resolve_external_file_path(path, protected_roots, "remote-web サムネイルキャッシュ")?;
        let connection = Connection::open(&path).map_err(|error| {
            format!(
                "remote-web サムネイルキャッシュを開けません ({}): {error}",
                path.display()
            )
        })?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;
                 PRAGMA busy_timeout=5000;
                 CREATE TABLE IF NOT EXISTS thumbnails (
                    cache_key TEXT PRIMARY KEY,
                    source_mtime_ns TEXT NOT NULL,
                    source_size INTEGER NOT NULL,
                    generation_size INTEGER NOT NULL,
                    thumb_data BLOB NOT NULL,
                    created_at INTEGER NOT NULL DEFAULT (unixepoch())
                 );",
            )
            .map_err(|error| format!("サムネイルキャッシュを初期化できません: {error}"))?;
        Ok(Self {
            path,
            connection: Mutex::new(connection),
        })
    }

    fn load(&self, cache_key: &str) -> Result<Option<Arc<Vec<u8>>>, ThumbnailServiceError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ThumbnailServiceError::Synchronization)?;
        let bytes = connection
            .query_row(
                "SELECT thumb_data FROM thumbnails WHERE cache_key = ?1",
                params![cache_key],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(|error| ThumbnailServiceError::Db(error.to_string()))?;
        Ok(bytes.map(Arc::new))
    }

    fn store(
        &self,
        cache_key: &str,
        source_mtime_ns: u128,
        source_size: u64,
        generation_size: u32,
        bytes: &[u8],
    ) -> Result<(), ThumbnailServiceError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ThumbnailServiceError::Synchronization)?;
        connection
            .execute(
                "INSERT OR REPLACE INTO thumbnails
                 (cache_key, source_mtime_ns, source_size, generation_size, thumb_data)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    cache_key,
                    source_mtime_ns.to_string(),
                    i64::try_from(source_size).unwrap_or(i64::MAX),
                    generation_size,
                    bytes,
                ],
            )
            .map_err(|error| ThumbnailServiceError::Db(error.to_string()))?;
        Ok(())
    }
}

struct Flight {
    result: Mutex<Option<Result<Arc<GeneratedThumbnail>, ThumbnailServiceError>>>,
    ready: Condvar,
}

impl Flight {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
            ready: Condvar::new(),
        }
    }

    fn complete(&self, result: Result<Arc<GeneratedThumbnail>, ThumbnailServiceError>) {
        if let Ok(mut slot) = self.result.lock() {
            *slot = Some(result);
            self.ready.notify_all();
        }
    }

    fn wait(&self) -> Result<Arc<GeneratedThumbnail>, ThumbnailServiceError> {
        let mut slot = self
            .result
            .lock()
            .map_err(|_| ThumbnailServiceError::Synchronization)?;
        while slot.is_none() {
            slot = self
                .ready
                .wait(slot)
                .map_err(|_| ThumbnailServiceError::Synchronization)?;
        }
        slot.as_ref().expect("flight result checked above").clone()
    }
}

struct GenerationGate {
    limit: usize,
    active: Mutex<usize>,
    available: Condvar,
}

impl GenerationGate {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            active: Mutex::new(0),
            available: Condvar::new(),
        }
    }

    fn enter(&self) -> Result<GenerationPermit<'_>, ThumbnailServiceError> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| ThumbnailServiceError::Synchronization)?;
        while *active >= self.limit {
            active = self
                .available
                .wait(active)
                .map_err(|_| ThumbnailServiceError::Synchronization)?;
        }
        *active += 1;
        Ok(GenerationPermit { gate: self })
    }
}

struct GenerationPermit<'a> {
    gate: &'a GenerationGate,
}

impl Drop for GenerationPermit<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.gate.active.lock() {
            *active = active.saturating_sub(1);
            self.gate.available.notify_one();
        }
    }
}

pub fn thumbnail_cache_key(
    relative_identity: &str,
    source_mtime_ns: u128,
    source_size: u64,
    generation_size: u32,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"remote-thumb-v1\0");
    hasher.update(relative_identity.as_bytes());
    hasher.update(b"\0");
    hasher.update(source_mtime_ns.to_le_bytes());
    hasher.update(source_size.to_le_bytes());
    hasher.update(generation_size.to_le_bytes());
    format!("{:x}", hasher.finalize())
}

fn generate_thumbnail(
    path: &Path,
    generation_size: u32,
) -> Result<GeneratedThumbnail, ThumbnailServiceError> {
    let total_started = Instant::now();
    let decode_started = Instant::now();
    let source = image_support::decode_oriented(path).ok_or(ThumbnailServiceError::Decode)?;
    let decode_ms = duration_ms(decode_started.elapsed());
    let (source_width, source_height) = source.dimensions();

    let resize_started = Instant::now();
    let longest = source_width.max(source_height).max(1);
    let scale = (generation_size as f64 / longest as f64).min(1.0);
    let output_width = ((source_width as f64 * scale).round() as u32).max(1);
    let output_height = ((source_height as f64 * scale).round() as u32).max(1);
    let resized = if output_width == source_width && output_height == source_height {
        source
    } else {
        image_support::resize_exact(&source, output_width, output_height)
    };
    let resize_ms = duration_ms(resize_started.elapsed());

    let encode_started = Instant::now();
    let rgba = resized.to_rgba8();
    let bytes = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
        .encode(THUMB_WEBP_QUALITY)
        .to_vec();
    let webp_encode_ms = duration_ms(encode_started.elapsed());
    Ok(GeneratedThumbnail {
        bytes: Arc::new(bytes),
        metrics: GenerationMetrics {
            source_width,
            source_height,
            output_width,
            output_height,
            decode_ms,
            resize_ms,
            webp_encode_ms,
            total_ms: duration_ms(total_started.elapsed()),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn key_changes_with_mtime_size_and_generation_size() {
        let base = thumbnail_cache_key("fav/page.jpg", 100, 200, 512);
        assert_ne!(base, thumbnail_cache_key("fav/page.jpg", 101, 200, 512));
        assert_ne!(base, thumbnail_cache_key("fav/page.jpg", 100, 201, 512));
        assert_ne!(base, thumbnail_cache_key("fav/page.jpg", 100, 200, 256));
    }

    #[test]
    fn concurrent_requests_generate_the_same_key_once() {
        let temp = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let generator_calls = Arc::clone(&calls);
        let generator: Arc<GenerateFn> = Arc::new(move |_path, _size| {
            generator_calls.fetch_add(1, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(40));
            Ok(GeneratedThumbnail {
                bytes: Arc::new(b"generated-webp".to_vec()),
                metrics: GenerationMetrics {
                    source_width: 10,
                    source_height: 10,
                    output_width: 10,
                    output_height: 10,
                    decode_ms: 1.0,
                    resize_ms: 1.0,
                    webp_encode_ms: 1.0,
                    total_ms: 3.0,
                },
            })
        });
        let service = Arc::new(
            ThumbnailService::open_with_generator(
                &temp.path().join("thumbs.db"),
                &[],
                4,
                generator,
            )
            .unwrap(),
        );
        let barrier = Arc::new(Barrier::new(8));
        let mut threads = Vec::new();
        for _ in 0..8 {
            let service = Arc::clone(&service);
            let barrier = Arc::clone(&barrier);
            threads.push(thread::spawn(move || {
                barrier.wait();
                service
                    .load_or_generate("same-key", Path::new("unused"), 1, 2, 512)
                    .unwrap()
                    .bytes
            }));
        }
        for thread in threads {
            assert_eq!(thread.join().unwrap().as_slice(), b"generated-webp");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
