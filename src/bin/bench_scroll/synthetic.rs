//! Headless S1 comparison. This uses the product's decoder/catalog worker entry
//! point, but never starts the GUI, reads a user profile, or purges OS caches.
use super::*;
use std::io::Write;
use std::sync::Condvar;
use std::time::Duration;

use mimageviewer::cover_identity::CoverIdentity;
use mimageviewer::video_pins::VideoPinDb;
use serde_json::json;

const COUNT: usize = 10_000;
const VISIBLE: usize = 20;
const PREFETCH: usize = 80;
const SCROLL: usize = COUNT / 2;
const PINS: usize = 200;

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
fn key(index: usize) -> String {
    format!("page-{index:05}.png")
}

#[derive(Clone, Copy)]
enum Shape {
    Full,
    Empty,
    Stale,
    Partial,
}
impl Shape {
    fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Empty => "empty",
            Self::Stale => "stale90",
            Self::Partial => "partial50",
        }
    }
    fn includes(self, i: usize) -> bool {
        match self {
            Self::Empty => false,
            Self::Partial => i % 2 == 0,
            _ => true,
        }
    }
    fn stale(self, i: usize) -> bool {
        matches!(self, Self::Stale) && i % 10 != 0
    }
    fn expected_hits(self, start: usize) -> usize {
        (start..start + VISIBLE)
            .filter(|&i| self.includes(i) && !self.stale(i))
            .count()
    }
}

struct Fixture {
    folder: PathBuf,
    cache_dir: PathBuf,
    stamps: Vec<(i64, i64)>,
    webp: Vec<u8>,
}

fn prepare(root: &Path) -> Fixture {
    // create_dir deliberately rejects an existing directory: never mix a fixture
    // with user data or silently replace a previous measurement.
    std::fs::create_dir(root)
        .expect("synthetic root must be a NEW directory (create its parent first)");
    let root = root.canonicalize().expect("canonical synthetic directory");
    let data = root.join("data");
    std::fs::create_dir(&data).unwrap();
    mimageviewer::data_dir::DATA_DIR
        .set(data)
        .expect("fresh benchmark process data directory");
    let folder = root.join("sources");
    let cache_dir = root.join("catalogs");
    std::fs::create_dir(&folder).unwrap();
    std::fs::create_dir(&cache_dir).unwrap();
    // Deterministic photo-sized entropy, not an unrealistically tiny solid WebP.
    let mut state = 0x800_u32;
    let image = image::RgbImage::from_fn(192, 256, |_, _| {
        let mut channel = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        };
        image::Rgb([channel(), channel(), channel()])
    });
    let image = image::DynamicImage::ImageRgb8(image);
    let webp = catalog::encode_thumb_webp(&image, 256, 75.0)
        .expect("fixture WebP")
        .0;
    let template = root.join("source.png");
    image.save(&template).unwrap();
    let mut stamps = vec![(1_700_000_000, 123_456); COUNT];
    // Only the two exercised pages need source files; the catalog still has
    // 10,000 real BLOB rows, including items outside both working sets.
    for start in [0, SCROLL] {
        for (i, stamp) in stamps
            .iter_mut()
            .enumerate()
            .skip(start)
            .take(VISIBLE + PREFETCH)
        {
            let path = folder.join(key(i));
            std::fs::copy(&template, &path).unwrap();
            let meta = std::fs::metadata(&path).unwrap();
            *stamp = (mtime_secs(&meta), meta.len() as i64);
        }
    }
    Fixture {
        folder,
        cache_dir,
        stamps,
        webp,
    }
}

fn populate(f: &Fixture, cache_dir: &Path, shape: Shape) {
    let db = CatalogDb::open(cache_dir, &f.folder).expect("create fixture catalog");
    for i in 0..COUNT {
        if !shape.includes(i) {
            continue;
        }
        let (mtime, size) = f.stamps[i];
        db.save(
            &key(i),
            mtime - i64::from(shape.stale(i)),
            size,
            192,
            256,
            Some((192, 256)),
            &f.webp,
            &CoverIdentity::Page(mimageviewer::adjustment_db::normalize_path(
                &f.folder.join(key(i)),
            )),
            Default::default(),
        )
        .unwrap();
    }
}

// A persistent, bounded pool with visible-first queues. A condition variable
// wakes idle workers; no polling sleep or product executable is involved.
struct WorkerPool {
    queue: Arc<(Mutex<(Vec<LoadRequest>, bool)>, Condvar)>,
    rx: mpsc::Receiver<ThumbMsg>,
    workers: Vec<std::thread::JoinHandle<()>>,
    map: Arc<RwLock<HashMap<String, catalog::CacheEntry>>>,
}

impl WorkerPool {
    fn new(
        threads: usize,
        db: Arc<CatalogDb>,
        map: Arc<RwLock<HashMap<String, catalog::CacheEntry>>>,
    ) -> Self {
        let queue = Arc::new((
            Mutex::new((Vec::<LoadRequest>::new(), false)),
            Condvar::new(),
        ));
        let (tx, rx) = mpsc::channel();
        let mut workers = Vec::new();
        for _ in 0..threads {
            let queue = Arc::clone(&queue);
            let tx = tx.clone();
            let db = Arc::clone(&db);
            let map = Arc::clone(&map);
            workers.push(std::thread::spawn(move || {
                let done = Arc::new(AtomicUsize::new(0));
                let stats = Arc::new(Mutex::new(ThumbStats::new()));
                let keep_start = Arc::new(AtomicUsize::new(0));
                let keep_end = Arc::new(AtomicUsize::new(COUNT));
                let decision = CacheDecision {
                    policy: CachePolicy::Off,
                    threshold_ms: 25,
                    size_threshold: 2 * 1024 * 1024,
                    webp_always: false,
                    pdf_always: false,
                    zip_always: false,
                    cache_book_cover_only: false,
                };
                loop {
                    let mut req = {
                        let (lock, wake) = &*queue;
                        let mut guard = lock.lock().unwrap();
                        while guard.0.is_empty() && !guard.1 {
                            guard = wake.wait(guard).unwrap();
                        }
                        if guard.1 {
                            break;
                        }
                        // push order is reversed so pop takes visible items first.
                        guard.0.pop().unwrap()
                    };
                    process_load_request(
                        &mut req,
                        &map,
                        &tx,
                        Some(&db),
                        256,
                        75,
                        256,
                        decision,
                        &done,
                        &stats,
                        None,
                        &keep_start,
                        &keep_end,
                        None,
                        None,
                        None,
                        None,
                        None,
                    );
                }
            }));
        }
        Self {
            queue,
            rx,
            workers,
            map,
        }
    }

    fn page(&self, f: &Fixture, mode: CacheMode, start: usize) -> serde_json::Value {
        let started = Instant::now();
        let (lock, wake) = &*self.queue;
        let mut old_fixed_misses = 0;
        {
            let mut queue = lock.lock().unwrap();
            assert!(queue.0.is_empty());
            for i in (start..start + VISIBLE + PREFETCH).rev() {
                let mut req = LoadRequest::default();
                req.idx = i;
                req.path = f.folder.join(key(i));
                (req.mtime, req.file_size) = f.stamps[i];
                req.priority = i < start + VISIBLE;
                mode.configure_request(&mut req);
                if mode == CacheMode::Memory {
                    // Before S1, an ordinary Fixed memory miss went straight to
                    // source generation. Do not give this old baseline the new
                    // per-key DB lookup. Fixtures have no Legacy/crop/Parent path.
                    let map = self.map.read().unwrap();
                    old_fixed_misses +=
                        usize::from(configure_old_fixed_miss(&mut req, map.get(&key(i))));
                }
                queue.0.push(req);
            }
        }
        wake.notify_all();
        let mut received = HashSet::new();
        let mut visible = HashSet::new();
        let mut hits = 0;
        let mut failures = 0;
        let mut first = None;
        let mut all_visible = None;
        let mut visible_hits = 0;
        while received.len() < VISIBLE + PREFETCH {
            let msg = self
                .rx
                .recv_timeout(Duration::from_secs(120))
                .expect("worker endpoint within 120 seconds");
            if msg.pixels().is_some() && received.insert(msg.idx) {
                let hit = msg.pixels().unwrap().origin.from_cache();
                hits += usize::from(hit);
                if msg.idx < start + VISIBLE {
                    visible_hits += usize::from(hit);
                    visible.insert(msg.idx);
                    first.get_or_insert_with(|| ms(started));
                    if visible.len() == VISIBLE {
                        all_visible = Some(ms(started));
                    }
                }
            } else if matches!(
                msg.payload,
                mimageviewer::thumb_loader::ThumbMsgPayload::Failed { .. }
                    | mimageviewer::thumb_loader::ThumbMsgPayload::Canceled
                    | mimageviewer::thumb_loader::ThumbMsgPayload::NoArt
                    | mimageviewer::thumb_loader::ThumbMsgPayload::CacheOnlyMiss
            ) && received.insert(msg.idx)
            {
                failures += 1;
            }
        }
        assert_eq!(failures, 0, "fixture must produce real pixels");
        json!({"first_visible_ms": first.unwrap(), "all_visible_ms": all_visible.unwrap(),
            "all_prefetch_ms": ms(started), "cache_hits": hits, "visible_cache_hits": visible_hits,
            "received": received.len(), "failures": failures,
            "old_fixed_miss_source_requests": old_fixed_misses})
    }
}

fn configure_old_fixed_miss(req: &mut LoadRequest, entry: Option<&catalog::CacheEntry>) -> bool {
    let miss =
        !entry.is_some_and(|entry| entry.mtime == req.mtime && entry.file_size == req.file_size);
    if miss {
        req.source_policy = mimageviewer::thumb_loader::LoadSourcePolicy::SourceOnly;
    }
    miss
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        let (lock, wake) = &*self.queue;
        lock.lock().unwrap().1 = true;
        wake.notify_all();
        for worker in self.workers.drain(..) {
            worker.join().expect("benchmark worker");
        }
    }
}

fn emit(out: &mut std::fs::File, row: serde_json::Value) {
    println!("{row}");
    writeln!(out, "{row}").unwrap();
    out.flush().unwrap();
}

fn catalog_cases(args: &Args, f: &Fixture, out: &mut std::fs::File) {
    let mut counts = vec![1, args.threads];
    counts.sort_unstable();
    counts.dedup();
    for shape in [Shape::Full, Shape::Empty, Shape::Stale, Shape::Partial] {
        let dir = f.cache_dir.join(shape.label());
        std::fs::create_dir(&dir).unwrap();
        populate(f, &dir, shape);
        for threads in &counts {
            for repetition in 0..args.repetitions {
                // Rotate ordering to avoid always giving one mode the warmest OS cache.
                let modes = [CacheMode::Memory, CacheMode::Db, CacheMode::Source];
                for mode in modes.iter().cycle().skip(repetition % 3).take(3).copied() {
                    for connection in ["fresh", "warm"] {
                        let opened = Instant::now();
                        let db = Arc::new(CatalogDb::open(&dir, &f.folder).unwrap());
                        let open_ms = ms(opened);
                        if connection == "warm" {
                            for start in [0, SCROLL] {
                                for i in start..start + VISIBLE {
                                    db.load_one(&key(i)).unwrap();
                                }
                            }
                        }
                        let prepare = Instant::now();
                        let map = if mode == CacheMode::Memory {
                            db.load_all().unwrap()
                        } else {
                            HashMap::new()
                        };
                        let map_entries = map.len();
                        let prepare_ms = ms(prepare);
                        let map = Arc::new(RwLock::new(map));
                        let pool_started = Instant::now();
                        let pool = WorkerPool::new(*threads, db, Arc::clone(&map));
                        let spawn_ms = ms(pool_started);
                        let initial = pool.page(f, mode, 0);
                        let scroll = pool.page(f, mode, SCROLL);
                        let expected = if mode == CacheMode::Source {
                            0
                        } else {
                            shape.expected_hits(0)
                        };
                        assert_eq!(
                            initial["visible_cache_hits"].as_u64().unwrap() as usize,
                            expected
                        );
                        assert_eq!(
                            scroll["visible_cache_hits"].as_u64().unwrap() as usize,
                            expected
                        );
                        for (start, page) in [(0, &initial), (SCROLL, &scroll)] {
                            let old_misses = if mode == CacheMode::Memory {
                                (start..start + VISIBLE + PREFETCH)
                                    .filter(|&i| !shape.includes(i) || shape.stale(i))
                                    .count()
                            } else {
                                0
                            };
                            assert_eq!(
                                page["old_fixed_miss_source_requests"].as_u64().unwrap() as usize,
                                old_misses
                            );
                        }
                        assert_eq!(
                            map.read().unwrap().len(),
                            map_entries,
                            "DB lookup must not populate the memory map"
                        );
                        emit(
                            out,
                            json!({"kind":"catalog", "shape":shape.label(), "mode":mode.label(),
                            "connection":connection, "workers":threads, "repetition":repetition,
                            "catalog_rows":map_entries_for_shape(shape), "blob_bytes":f.webp.len(),
                            "open_ms":open_ms, "prepare_ms":prepare_ms, "spawn_ms":spawn_ms,
                            "ui_prepare_ms":open_ms + prepare_ms,
                            "initial_ready_ms":open_ms + prepare_ms + spawn_ms + initial["all_visible_ms"].as_f64().unwrap(),
                            "memory_entries_before":map_entries, "memory_entries_after":map.read().unwrap().len(),
                            "initial":initial, "scroll":scroll}),
                        );
                        drop(pool);
                    }
                }
            }
        }
    }
}

fn map_entries_for_shape(shape: Shape) -> usize {
    (0..COUNT).filter(|&i| shape.includes(i)).count()
}

// Keep the already_seeded comparison identical to seed_folder_video_pin_thumbs:
// source dimensions / provenance are deliberately NOT added to this predicate.
fn seed_same(
    entry: &catalog::CacheEntry,
    mtime: i64,
    size: i64,
    webp: &[u8],
    cover: &CoverIdentity,
) -> bool {
    entry.mtime == mtime
        && entry.file_size == size
        && entry.jpeg_data == webp
        && entry.cover == *cover
}

fn seed_cases(args: &Args, f: &Fixture, out: &mut std::fs::File) {
    let video = VideoPinDb::open_at(&mimageviewer::data_dir::get().join("video_pins.db")).unwrap();
    let paths: Vec<_> = (0..PINS)
        .map(|i| f.folder.join(format!("video-{i:05}.mp4")))
        .collect();
    for path in &paths {
        video.set_pin(path, 1.0, &f.webp).unwrap();
    }
    for background in [0, COUNT] {
        for shape in ["empty", "same", "partial"] {
            for repetition in 0..args.repetitions {
                for mode in [CacheMode::Memory, CacheMode::Db] {
                    for connection in ["fresh", "warm"] {
                        let dir = f.cache_dir.join(format!(
                            "seed-{background}-{shape}-{repetition}-{}-{connection}",
                            mode.label()
                        ));
                        std::fs::create_dir(&dir).unwrap();
                        // Copy only a CLOSED fixture: last-connection close has checkpointed
                        // its WAL. The zero-background case includes a truly empty catalog.
                        let db_path = catalog::db_path_for(&dir, &f.folder);
                        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
                        if background > 0 {
                            std::fs::copy(
                                catalog::db_path_for(&f.cache_dir.join("full"), &f.folder),
                                &db_path,
                            )
                            .unwrap();
                        }
                        {
                            let db = CatalogDb::open(&dir, &f.folder).unwrap();
                            for (i, path) in paths.iter().enumerate() {
                                if shape == "same" || (shape == "partial" && i % 2 == 0) {
                                    assert!(
                                        db.save_seeded_folder_bytes(
                                            &format!("seed-pin-{i}"),
                                            123,
                                            456,
                                            None,
                                            &f.webp,
                                            &CoverIdentity::Page(
                                                mimageviewer::adjustment_db::normalize_path(path)
                                            ),
                                            Default::default()
                                        )
                                        .unwrap()
                                    );
                                }
                            }
                        }
                        let open = Instant::now();
                        let db = CatalogDb::open(&dir, &f.folder).unwrap();
                        let open_ms = ms(open);
                        if connection == "warm" {
                            for i in 0..PINS {
                                db.load_one(&format!("seed-pin-{i}")).unwrap();
                            }
                        }
                        let prepare = Instant::now();
                        let mut map = if mode == CacheMode::Memory {
                            db.load_all().unwrap()
                        } else {
                            HashMap::new()
                        };
                        let prepare_ms = ms(prepare);
                        let started = Instant::now();
                        let mut reads = 0;
                        let mut writes = 0;
                        for (i, path) in paths.iter().enumerate() {
                            let key = format!("seed-pin-{i}");
                            let webp = video.lookup(path).expect("pin exists").thumb_webp;
                            let cover = CoverIdentity::Page(
                                mimageviewer::adjustment_db::normalize_path(path),
                            );
                            let already_seeded = if mode == CacheMode::Memory {
                                map.get(&key)
                                    .is_some_and(|entry| seed_same(entry, 123, 456, &webp, &cover))
                            } else {
                                reads += 1;
                                db.matches_video_seed(&key, 123, 456, &webp, &cover)
                                    .unwrap()
                            };
                            if already_seeded {
                                continue;
                            }
                            assert!(
                                db.save_seeded_folder_bytes(
                                    &key,
                                    123,
                                    456,
                                    None,
                                    &webp,
                                    &cover,
                                    Default::default()
                                )
                                .unwrap()
                            );
                            writes += 1;
                            map.insert(
                                key,
                                catalog::CacheEntry {
                                    mtime: 123,
                                    file_size: 456,
                                    jpeg_data: webp,
                                    cover,
                                    auto_material: Default::default(),
                                    source_dims: None,
                                    layout_dims: None,
                                    folder_provenance: Some(catalog::FolderThumbProvenance::Seeded),
                                    selection_proof: None,
                                },
                            );
                        }
                        let seed_ms = ms(started);
                        assert_eq!(
                            writes,
                            match shape {
                                "empty" => PINS,
                                "same" => 0,
                                _ => PINS / 2,
                            }
                        );
                        emit(
                            out,
                            json!({"kind":"seed", "shape":shape, "mode":mode.label(),
                        "connection":connection, "repetition":repetition, "pins":PINS,
                        "background_rows":background, "catalog_reads":reads, "writes":writes,
                        "open_ms":open_ms, "prepare_ms":prepare_ms, "seed_ms":seed_ms,
                        "ui_prepare_ms":open_ms + prepare_ms + seed_ms}),
                        );
                        drop(db);
                        // Delete only these exact disposable fixture files, never recurse.
                        assert!(db_path.starts_with(&f.cache_dir));
                        for suffix in ["", "-wal", "-shm"] {
                            let mut name = db_path.as_os_str().to_os_string();
                            name.push(suffix);
                            let path = PathBuf::from(name);
                            if path.exists() {
                                std::fs::remove_file(path).unwrap();
                            }
                        }
                    }
                }
            }
        }
    }
}

pub(super) fn run(args: &Args, root: &Path) {
    let f = prepare(root);
    let mut out = std::fs::File::create(root.join("results.jsonl")).unwrap();
    let mut worker_counts = vec![1, args.threads];
    worker_counts.sort_unstable();
    worker_counts.dedup();
    emit(
        &mut out,
        json!({"kind":"protocol", "version":1, "items":COUNT,
        "visible":VISIBLE, "prefetch":PREFETCH, "scroll_index":SCROLL, "pins":PINS,
        "blob_bytes":f.webp.len(), "repetitions":args.repetitions, "worker_counts":worker_counts,
        "source_policy":"source mode explicitly uses SourceOnly; catalog cases use CachePolicy::Off to preserve DB shape",
        "memory_baseline":"old Fixed worker semantics for ordinary PNG: matching memory rows use cache; missing/stale rows use SourceOnly without S1 DB lookup",
        "connection_note":"fresh SQLite connection versus reused visible-key-primed connection; OS cache is not cold",
        "seed_note":"empty/same/partial PIN rows with both zero and 10000 ordinary rows; 200 actual VideoPinDb lookups"}),
    );
    catalog_cases(args, &f, &mut out);
    seed_cases(args, &f, &mut out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_fixed_baseline_uses_source_for_missing_or_stale_rows_only() {
        let entry = catalog::CacheEntry {
            mtime: 123,
            file_size: 456,
            jpeg_data: Vec::new(),
            cover: CoverIdentity::Unknown,
            auto_material: Default::default(),
            source_dims: None,
            layout_dims: None,
            folder_provenance: None,
            selection_proof: None,
        };
        for (mtime, size, present, expected_source) in [
            (123, 456, true, false),
            (124, 456, true, true),
            (123, 457, true, true),
            (123, 456, false, true),
        ] {
            let mut req = LoadRequest::default();
            req.mtime = mtime;
            req.file_size = size;
            assert_eq!(
                configure_old_fixed_miss(&mut req, present.then_some(&entry)),
                expected_source
            );
            assert_eq!(req.source_policy.bypasses_cache(), expected_source);
        }
    }
}
