//! Opt-in, headless measurement. Never constructs App or starts the product/PDF pool.
//! See backlog §1.668 for conditions, limitations and the exact command.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::{Duration, Instant};

use crate::catalog::{CacheEntry, CatalogDb};
use crate::collection_store::*;
use crate::grid_item::GridItem;
use crate::thumb_loader::{CacheDecision, LoadRequest, ThumbLoadOrigin, process_load_request};

const COUNT: usize = 2_000;
const VISIBLE: usize = 40;
const REPEATS: usize = 5;

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000.0
}

// Data, catalogs and results all stay on the worktree volume, even if TEMP is on C:.
struct Sandbox {
    root: PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Sandbox {
    fn new() -> Self {
        let lock = crate::data_dir::test_override_lock();
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/section1668");
        std::fs::create_dir_all(&parent).unwrap();
        let root = tempfile::Builder::new()
            .prefix("run-")
            .tempdir_in(parent)
            .unwrap()
            .keep();
        let data = root.join("data");
        std::fs::create_dir(&data).unwrap();
        crate::data_dir::set_test_override(Some(data.clone()));
        assert_eq!(crate::data_dir::get(), data);
        Self { root, _lock: lock }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        crate::data_dir::set_test_override(None);
    }
}

fn jpeg(seed: u32) -> Vec<u8> {
    let mut rng = seed + 1;
    let image = image::RgbImage::from_fn(2048, 1536, |x, y| {
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let noise = (rng >> 27) as u8;
        image::Rgb([
            ((x / 8 + y / 16) % 224) as u8 + noise,
            ((y / 7 + seed * 17) % 224) as u8 + noise,
            ((x / 12 + y / 11) % 224) as u8 + noise,
        ])
    });
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .encode_image(&image)
        .unwrap();
    out
}

fn fixture_zip(jpegs: &[Vec<u8>]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (index, bytes) in jpegs.iter().enumerate() {
        zip.start_file(
            format!("{index:03}.jpg"),
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn fixture_pdf(jpegs: &[Vec<u8>]) -> Vec<u8> {
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    for bytes in jpegs {
        let image_id = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Image", "Width" => 2048,
                "Height" => 1536, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
                "Filter" => "DCTDecode",
            },
            bytes.clone(),
        ));
        let content = doc.add_object(Stream::new(
            dictionary! {},
            b"q 768 0 0 576 0 0 cm /Im0 Do Q".to_vec(),
        ));
        kids.push(Object::Reference(doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 768.into(), 576.into()],
            "Contents" => content,
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image_id } },
        })));
    }
    doc.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages", "Kids" => kids, "Count" => jpegs.len() as i64,
        }
        .into(),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

fn request(entry: &PreparedCollectionEntry, idx: usize, full: bool) -> LoadRequest {
    let (mtime, size) = entry.display_meta.unwrap();
    let mut req = super::super::make_load_request(
        &entry.item,
        idx,
        mtime,
        size,
        false,
        None,
        None,
        3,
        &HashMap::new(),
        &HashMap::new(),
        None,
        None,
        None,
        None,
        full,
    )
    .unwrap();
    req.priority = true;
    req.items_gen = 1;
    req
}

#[derive(serde::Serialize)]
struct Batch {
    ready_ms: f64,
    settled_ms: f64,
    cache: usize,
    source: usize,
    pixel_sizes: Vec<[usize; 2]>,
    #[serde(skip)]
    ready_at: Instant,
}

// Same product request builder and process_load_request; bounded test dispatcher only.
// Six regular workers + two heavy-I/O workers, matching spawn_thumbnail_workers at 8 threads.
fn batch(
    entries: &[PreparedCollectionEntry],
    range: std::ops::Range<usize>,
    full: bool,
    map: &RwLock<HashMap<String, CacheEntry>>,
    catalog: Option<&Arc<CatalogDb>>,
) -> Batch {
    let mut light = Vec::new();
    let mut heavy = Vec::new();
    for idx in range.clone() {
        let req = request(&entries[idx], idx, full);
        if entries[idx].item.is_heavy_io() {
            heavy.push(req);
        } else {
            light.push(req);
        }
    }
    light.reverse();
    heavy.reverse();
    let light = Mutex::new(light);
    let heavy = Mutex::new(heavy);
    let done = Arc::new(AtomicUsize::new(0));
    let stats = Arc::new(Mutex::new(crate::stats::ThumbStats::default()));
    let keep_start = Arc::new(AtomicUsize::new(range.start));
    let keep_end = Arc::new(AtomicUsize::new(range.end));
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let mut cache = 0;
    let mut source = 0;
    let mut pixel_sizes = HashSet::new();
    let start = Instant::now();
    let ready_at = std::thread::scope(|scope| {
        for worker in 0..8 {
            let queue = if worker < 6 { &light } else { &heavy };
            let tx = tx.clone();
            let done = &done;
            let stats = &stats;
            let keep_start = &keep_start;
            let keep_end = &keep_end;
            let cancel = &cancel;
            scope.spawn(move || {
                loop {
                    let next = queue.lock().unwrap().pop();
                    let Some(mut req) = next else { break };
                    process_load_request(
                        &mut req,
                        map,
                        &tx,
                        catalog,
                        512,
                        75,
                        256,
                        CacheDecision {
                            policy: crate::settings::CachePolicy::Always,
                            threshold_ms: 0,
                            size_threshold: 0,
                            webp_always: true,
                            pdf_always: true,
                            zip_always: true,
                        },
                        done,
                        stats,
                        Some(cancel),
                        keep_start,
                        keep_end,
                        None,
                        None,
                        None,
                        None,
                        None,
                    );
                }
            });
        }
        let mut seen = HashSet::new();
        while seen.len() < range.len() {
            let msg = rx
                .recv_timeout(Duration::from_secs(120))
                .expect("thumbnail reply");
            assert!(range.contains(&msg.idx));
            if msg.is_finalized() {
                continue;
            }
            let pixels = msg
                .pixels()
                .expect("real decoded pixels, not failed/canceled");
            assert!(!pixels.image.pixels.is_empty());
            pixel_sizes.insert(pixels.image.size);
            assert!(seen.insert(msg.idx), "one result per visible cell");
            if pixels.origin == ThumbLoadOrigin::UpgradeableCache {
                cache += 1;
            } else {
                assert!(!pixels.origin.from_cache(), "unexpected cache type");
                source += 1;
            }
        }
        Instant::now()
    });
    assert_eq!(done.load(Ordering::Relaxed), range.len());
    let mut pixel_sizes = pixel_sizes.into_iter().collect::<Vec<_>>();
    pixel_sizes.sort_unstable();
    Batch {
        ready_ms: ready_at.duration_since(start).as_secs_f64() * 1_000.0,
        settled_ms: ms(start),
        cache,
        source,
        pixel_sizes,
        ready_at,
    }
}

fn seed_parents(
    entries: &[PreparedCollectionEntry],
    cache_dir: &Path,
) -> HashMap<String, CacheEntry> {
    let mut by_parent: HashMap<&Path, Vec<(String, String, i64, i64)>> = HashMap::new();
    for (idx, entry) in entries.iter().enumerate() {
        let full = request(entry, idx, true);
        let local = request(entry, idx, false);
        by_parent
            .entry(entry.source_path.parent().unwrap())
            .or_default()
            .push((
                crate::thumb_loader::cache_key_for_request(&local)
                    .unwrap()
                    .into_owned(),
                crate::thumb_loader::cache_key_for_request(&full)
                    .unwrap()
                    .into_owned(),
                full.mtime,
                full.file_size,
            ));
    }
    let mut map = HashMap::new();
    for (parent, keys) in by_parent {
        let db = CatalogDb::open_existing_read_only(cache_dir, parent)
            .unwrap()
            .unwrap();
        // Selected reads, not load_all of an unrelated large source folder.
        for (local, full, mtime, size) in keys {
            if let Some(row) = db.load_one(&local).unwrap()
                && row.mtime == mtime
                && row.file_size == size
            {
                map.insert(full, row);
            }
        }
    }
    map
}

#[test]
#[ignore = "headless SSD collection thumbnail measurement; generates ~4.5 GB; backlog 1.668"]
fn measure_collection_thumbnails_2000() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let sandbox = Sandbox::new();
    let root = &sandbox.root;
    println!("section1668 root={}", root.display());
    // No global services/App/startup/logging. Existing PDF test adapter calls real ipc_render
    // and PDFium in-process, bypassing product subprocess startup and document reuse.
    let images = (0..4).map(jpeg).collect::<Vec<_>>();
    let zip = fixture_zip(&images);
    let pdf = fixture_pdf(&images);
    let id = CollectionId::new();
    let mut entries = Vec::new();
    let mut pdf_backends = Vec::new();
    let mut total_bytes = 0_u64;
    for idx in 0..COUNT {
        let parent = root.join(format!("sources/parent-{:02}", idx / 100));
        std::fs::create_dir_all(&parent).unwrap();
        let (ext, kind, bytes) = match idx % 4 {
            0 | 1 => ("jpg", CollectionResolvedKind::Image, &images[idx % 4]),
            2 => ("zip", CollectionResolvedKind::Zip, &zip),
            _ => ("pdf", CollectionResolvedKind::Pdf, &pdf),
        };
        let path = parent.join(format!("item-{:03}.{ext}", idx % 100));
        std::fs::write(&path, bytes).unwrap();
        // pdf_meta catch-up still uses basename even for aggregate catalogs. All duplicate
        // basenames in this fixture have identical contents and an identical known stamp.
        // This makes the preseed below valid for every parent and prevents background jobs.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            )
            .unwrap();
        total_bytes += bytes.len() as u64;
        if ext == "pdf" {
            pdf_backends.push(crate::pdf_loader::RemotePdfTestBackend::for_path(&path));
        }
        let reg = CollectionRegistration::from_trusted_path(&path, kind).unwrap();
        entries.push(CollectionEntry {
            id: CollectionEntryId::new(),
            collection_id: id,
            source_path: path,
            source_key: reg.source_key().clone(),
            resolved_kind: kind,
            manual_position: idx as u64,
        });
    }
    let snapshot = CollectionSnapshot {
        catalog_revision: 1,
        definition: CollectionDefinition {
            id,
            name: "section1668".into(),
            order_mode: CollectionOrderMode::Manual,
            standard_sort: crate::settings::SortOrder::Numeric,
            shuffle_seed: 0,
            revision: 1,
        },
        entries: entries.into(),
    };
    let runtime =
        CollectionStoreRuntime::start_at(crate::data_dir::get().join("collection.db")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match runtime.try_recv_event() {
            Some(CollectionRuntimeEvent::Ready(_)) => break,
            Some(event) => panic!("actor startup: {event:?}"),
            None => {}
        }
        assert!(Instant::now() < deadline, "actor startup deadline");
        std::thread::sleep(Duration::from_millis(5));
    }
    let client = runtime.client();
    let created = client
        .create_collection("section1668".into())
        .unwrap()
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    let registrations = snapshot
        .entries
        .iter()
        .map(|entry| {
            CollectionRegistration::from_trusted_path(&entry.source_path, entry.resolved_kind)
                .unwrap()
        })
        .collect();
    let snapshot = client
        .add_batch(created.collection_id(), created.revision(), registrations)
        .unwrap()
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap()
        .snapshot;
    let id = snapshot.collection_id();
    let settings = crate::settings::Settings::default();
    let cancel = AtomicBool::new(false);
    // Create empty, genuine edit/pin stores. Their normal read paths are included below.
    drop(crate::adjustment_db::AdjustmentDb::open().unwrap());
    drop(crate::export_crop::CropDb::open().unwrap());
    drop(crate::view_trim_db::ViewTrimDb::open().unwrap());
    drop(crate::mask_db::MaskDb::open().unwrap());
    drop(crate::conceal_db::ConcealDb::open().unwrap());
    drop(crate::comic_db::ComicDb::open().unwrap());
    drop(crate::local_adjust_db::LocalAdjustDb::open().unwrap());
    drop(crate::video_pins::VideoPinDb::open().unwrap());
    drop(crate::folder_thumb_pins::FolderThumbPinDb::open().unwrap());
    let available = crate::app::page_edit_snapshot::PageEditAvailability {
        adjustment: true,
        export_crop: true,
        view_trim: true,
        mask: true,
        conceal: true,
        comic: true,
        local_adjust: true,
    };
    let mut prepare = Vec::new();
    for _ in 0..REPEATS {
        let start = Instant::now();
        let install = crate::app::collection_grid::prepare_collection_grid_install(
            &snapshot,
            &settings.grid_display_order,
            &settings,
            &cancel,
            None,
            None,
            None,
            0,
            available,
            0,
        )
        .unwrap();
        let total = ms(start);
        assert_eq!(install.prepared.entries.len(), COUNT);
        // Separate probes of the same stage functions; not additive subdivisions of total.
        let start = Instant::now();
        let prepared = prepare_collection_snapshot(
            &snapshot,
            &settings.grid_display_order,
            &cancel,
            |_, _| {},
        )
        .unwrap();
        let classify = ms(start);
        let items = prepared
            .entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>();
        let start = Instant::now();
        crate::app::folder_scan::discover_aggregate_video_sidecars_while(
            &settings,
            &[],
            64,
            || true,
        )
        .unwrap();
        let sidecar = ms(start);
        let start = Instant::now();
        crate::app::page_edit_snapshot::PageEditSnapshot::load_and_project_stable(
            &items, available, &cancel,
        )
        .unwrap()
        .unwrap();
        let edit = ms(start);
        let start = Instant::now();
        let video =
            crate::video_pins::VideoPinDb::open_readonly(&crate::video_pins::VideoPinDb::db_path())
                .unwrap();
        video.lookup_webps_many(std::iter::empty::<&PathBuf>());
        let pins = crate::folder_thumb_pins::FolderThumbPinDb::open_readonly(
            &crate::folder_thumb_pins::FolderThumbPinDb::db_path(),
        )
        .unwrap();
        pins.lookup_many(items.iter().filter_map(GridItem::container_path));
        let pin = ms(start);
        prepare.push(
            serde_json::json!({"total_ms":total,"classify_probe_ms":classify,
            "sidecar_probe_ms":sidecar,"edit_db_probe_ms":edit,"pin_db_probe_ms":pin}),
        );
    }
    let prepared =
        prepare_collection_snapshot(&snapshot, &settings.grid_display_order, &cancel, |_, _| {})
            .unwrap();
    let entries = prepared.entries.as_ref();
    let cache_dir = root.join("catalogs");
    // Warm every source folder through product requests/workers, including basename keys.
    let start = Instant::now();
    for parent in 0..20 {
        let range = parent * 100..(parent + 1) * 100;
        let db = Arc::new(
            CatalogDb::open(
                &cache_dir,
                entries[range.start].source_path.parent().unwrap(),
            )
            .unwrap(),
        );
        let map = RwLock::new(HashMap::new());
        let result = batch(entries, range, false, &map, Some(&db));
        assert_eq!(result.source, 100);
        assert_eq!(db.load_all().unwrap().len(), 100);
    }
    let normal_warm_ms = ms(start);
    let synthetic = PathBuf::from(format!("__collection__/{id}"));
    let db = Arc::new(CatalogDb::open(&cache_dir, &synthetic).unwrap());
    // Isolate thumbnail timing from the unrelated asynchronous pdf_meta catch-up service.
    // All generated PDFs have exactly four pages; metadata does not seed thumbnail pixels.
    for entry in entries {
        if matches!(entry.item, GridItem::PdfFile(_)) {
            let (mtime, size) = entry.display_meta.unwrap();
            db.set_pdf_meta(
                entry.source_path.file_name().unwrap().to_str().unwrap(),
                mtime,
                size,
                4,
                false,
            )
            .unwrap();
        }
    }
    for entry in entries {
        if matches!(entry.item, GridItem::PdfFile(_)) {
            let (mtime, size) = entry.display_meta.unwrap();
            assert!(
                db.get_pdf_meta(
                    entry.source_path.file_name().unwrap().to_str().unwrap(),
                    mtime,
                    size
                )
                .unwrap()
                .is_some(),
                "every PDF must skip background catch-up"
            );
        }
    }
    drop(db);
    let mut rows = Vec::new();
    for repeat in 0..REPEATS {
        // Three disjoint windows: first viewport, far-away viewport, return after eviction.
        for mode in ["current", "B", "A"] {
            let open_start = Instant::now();
            let actor_start = Instant::now();
            let loaded = client
                .load_collection(id)
                .unwrap()
                .recv_timeout(Duration::from_secs(30))
                .unwrap()
                .unwrap();
            let actor_ms = ms(actor_start);
            let prepare_start = Instant::now();
            let install = crate::app::collection_grid::prepare_collection_grid_install(
                &loaded,
                &settings.grid_display_order,
                &settings,
                &cancel,
                None,
                None,
                None,
                0,
                available,
                0,
            )
            .unwrap();
            let prepare_ms = ms(prepare_start);
            let entries = install.prepared.entries.as_ref();
            let load_start = Instant::now();
            let catalog =
                (mode == "A").then(|| Arc::new(CatalogDb::open(&cache_dir, &synthetic).unwrap()));
            let map = match mode {
                "B" => seed_parents(entries, &cache_dir),
                "A" => catalog.as_ref().unwrap().load_all().unwrap(),
                _ => HashMap::new(),
            };
            let load_ms = ms(load_start);
            let cached_rows = map.len();
            assert_eq!(
                cached_rows,
                match mode {
                    "B" => COUNT,
                    "A" if repeat > 0 => VISIBLE * 2,
                    _ => 0,
                }
            );
            let map = RwLock::new(map);
            let first = batch(entries, 0..VISIBLE, true, &map, catalog.as_ref());
            let open_ready_ms = first.ready_at.duration_since(open_start).as_secs_f64() * 1_000.0;
            let away = batch(entries, 1000..1000 + VISIBLE, true, &map, catalog.as_ref());
            // Drop decoded pixels in batch. Retain encoded cache_map just like real scroll.
            let back = batch(entries, 0..VISIBLE, true, &map, catalog.as_ref());
            if mode == "current" {
                assert_eq!((first.source, back.source), (VISIBLE, VISIBLE));
                assert!(map.read().unwrap().is_empty());
            } else if mode == "B" || repeat > 0 {
                assert_eq!((first.cache, back.cache), (VISIBLE, VISIBLE));
            } else {
                assert_eq!(
                    (first.source, away.source, back.cache),
                    (VISIBLE, VISIBLE, VISIBLE)
                );
            }
            let row = serde_json::json!({"repeat":repeat,"mode":mode,"catalog_load_ms":load_ms,
                "actor_ms":actor_ms,"prepare_ms":prepare_ms,"open_ready_ms":open_ready_ms,"cached_rows":cached_rows,
                "first":first,"away":away,"back":back});
            println!("section1668 {row}");
            rows.push(row);
        }
    }
    let report = serde_json::json!({
        "count":COUNT,"visible":VISIBLE,"parents":20,"jpeg":1000,"zip":500,"pdf":500,
        "width":2048,"height":1536,"pages_per_book":4,"total_bytes":total_bytes,
        "jpeg_bytes":[images[0].len(),images[1].len()],"zip_bytes":zip.len(),"pdf_bytes":pdf.len(),
        "normal_folder_warm_ms":normal_warm_ms,"prepare":prepare,"rows":rows,
        "pdf_backend":"real PDFium / ipc_render, in-process test adapter, no document reuse or pool",
        "debug_assertions":cfg!(debug_assertions),
        "test_executable":std::env::current_exe().unwrap(),
    });
    std::fs::write(
        root.join("results.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!(
        "section1668 results={}",
        root.join("results.json").display()
    );
    assert!(
        crate::pdf_loader::pool_queue_snapshot().is_none(),
        "product PDF pool must never start"
    );
    runtime.shutdown_and_join();
    drop(pdf_backends);
}
