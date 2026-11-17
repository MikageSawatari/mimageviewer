//! Sparse listing records must never stand in for catalog existence.
use super::*;
use std::collections::HashMap;
use std::sync::RwLock;

fn run(
    req: &mut LoadRequest,
    db: &Arc<crate::catalog::CatalogDb>,
    map: &RwLock<HashMap<String, crate::catalog::CacheEntry>>,
) -> Vec<ThumbMsg> {
    run_with_policy(req, db, map, crate::settings::CachePolicy::Always)
}

fn run_with_policy(
    req: &mut LoadRequest,
    db: &Arc<crate::catalog::CatalogDb>,
    map: &RwLock<HashMap<String, crate::catalog::CacheEntry>>,
    policy: crate::settings::CachePolicy,
) -> Vec<ThumbMsg> {
    let (tx, rx) = mpsc::channel();
    let done = Arc::new(AtomicUsize::new(0));
    process_load_request(
        req,
        map,
        &tx,
        Some(db),
        64,
        75,
        64,
        CacheDecision {
            policy,
            threshold_ms: 0,
            size_threshold: 0,
            webp_always: true,
            zip_always: true,
            pdf_always: true,
            cache_book_cover_only: false,
        },
        &done,
        &Arc::new(Mutex::new(crate::stats::ThumbStats::default())),
        None,
        &Arc::new(AtomicUsize::new(0)),
        &Arc::new(AtomicUsize::new(10)),
        None,
        None,
        None,
        None,
        None,
    );
    assert_eq!(done.load(Ordering::Relaxed), 1);
    rx.try_iter().collect()
}

fn seed(db: &crate::catalog::CatalogDb, key: &str, bytes: &[u8]) {
    db.save(
        key,
        7,
        9,
        8,
        12,
        Some((8, 12)),
        bytes,
        &CoverIdentity::Unknown,
        crate::cover_auto_material::AutoMaterial::NotAutomatic,
    )
    .unwrap();
}

#[test]
fn fixed_db_hit_uses_exact_variant_without_source_or_memory_registration() {
    let temp = tempfile::tempdir().unwrap();
    let db =
        Arc::new(crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap());
    let bytes = crate::catalog::encode_thumb_webp(&image::DynamicImage::new_rgb8(8, 12), 64, 75.0)
        .unwrap()
        .0;
    for key in [
        "page.png",
        "page_0000",
        "pdfthumb:book.epub#pin:page|generation:7:9",
        "zipthumb:book.zip#pin:page",
        "pdfthumb:book.pdf",
    ] {
        let map = RwLock::new(HashMap::new());
        let mut req = LoadRequest {
            path: temp.path().join("absent.png"),
            mtime: 7,
            file_size: 9,
            cache_key_override: Some(key.into()),
            ..Default::default()
        };
        if key == "pdfthumb:book.pdf" {
            req.file_type_visibility
                .excluded_extensions
                .insert("gif".into());
            req.set_auto_cover_crop(Some(crate::cover_auto_crop::AutoCropIdentity::new(
                crate::settings::ReadingDirection::Rtl,
            )));
        }
        let storage_key = cache_key_for_request(&req).unwrap().into_owned();
        if key == "pdfthumb:book.pdf" {
            assert!(storage_key.contains("|types:"));
            assert!(storage_key.contains("|autocrop:"));
        }
        seed(&db, &storage_key, &bytes);
        let messages = run(&mut req, &db, &map);
        assert_eq!(messages.len(), 1, "{key}");
        assert_eq!(
            messages[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
        assert!(map.read().unwrap().is_empty());
        assert_eq!(db.load_one(&storage_key).unwrap().unwrap().jpeg_data, bytes);
    }
}

#[test]
fn fixed_stale_or_invalid_webp_regenerates_but_source_only_bypasses_valid_db() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("page.png");
    image::DynamicImage::new_rgb8(16, 8).save(&path).unwrap();
    let db =
        Arc::new(crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap());
    let bytes = crate::catalog::encode_thumb_webp(&image::DynamicImage::new_rgb8(8, 12), 64, 75.0)
        .unwrap()
        .0;
    for (stamp, data, policy) in [
        (8, bytes.as_slice(), LoadSourcePolicy::CacheOrSource),
        (
            7,
            b"broken webp".as_slice(),
            LoadSourcePolicy::CacheOrSource,
        ),
        (7, bytes.as_slice(), LoadSourcePolicy::SourceOnly),
    ] {
        seed(&db, "page.png", data);
        let map = RwLock::new(HashMap::new());
        let mut req = LoadRequest {
            path: path.clone(),
            mtime: stamp,
            file_size: 9,
            source_policy: policy,
            ..Default::default()
        };
        let messages = run(&mut req, &db, &map);
        assert_eq!(messages.len(), 2);
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::SourcePixels(_)
        ));
        assert!(messages[1].is_finalized());
        assert_eq!(map.read().unwrap()["page.png"].source_dims, Some((16, 8)));
    }
}

#[test]
fn fixed_retired_catalog_miss_does_not_read_old_row() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let db = Arc::new(crate::catalog::CatalogDb::open(&cache, temp.path()).unwrap());
    let path = temp.path().join("page.png");
    image::DynamicImage::new_rgb8(16, 8).save(&path).unwrap();
    let bytes = crate::catalog::encode_thumb_webp(&image::DynamicImage::new_rgb8(8, 12), 64, 75.0)
        .unwrap()
        .0;
    seed(&db, "page.png", &bytes);
    let delete = crate::catalog::CatalogAccess::for_cache_dir(&cache)
        .begin_delete(crate::catalog::CatalogDeleteOperation::All)
        .unwrap();
    delete.retire_connections();
    let mut req = LoadRequest {
        path,
        mtime: 7,
        file_size: 9,
        ..Default::default()
    };
    let map = RwLock::new(HashMap::new());
    let messages = run(&mut req, &db, &map);
    assert!(matches!(
        messages[0].payload,
        ThumbMsgPayload::SourcePixels(_)
    ));
    assert_eq!(messages[0].pixels().unwrap().source_dims, Some((16, 8)));
    assert!(messages[1].is_finalized());
    assert!(map.read().unwrap().is_empty());
    drop(delete);
}

#[test]
fn invalidation_after_fixed_read_cannot_repopulate_record_before_next_request() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("page.png");
    image::DynamicImage::new_rgb8(16, 8).save(&path).unwrap();
    let db =
        Arc::new(crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap());
    let bytes = crate::catalog::encode_thumb_webp(&image::DynamicImage::new_rgb8(8, 12), 64, 75.0)
        .unwrap()
        .0;
    seed(&db, "page.png", &bytes);
    let map = RwLock::new(HashMap::new());
    let mut req = LoadRequest {
        path,
        mtime: 7,
        file_size: 9,
        ..Default::default()
    };
    // Retain the old worker's unadopted result across invalidation completion.
    let old = run(&mut req, &db, &map);
    db.delete_one("page.png").unwrap();
    map.write().unwrap().remove("page.png"); // UI completion's memory cleanup.
    assert_eq!(
        old[0].pixels().unwrap().origin,
        ThumbLoadOrigin::UpgradeableCache
    );
    assert!(map.read().unwrap().is_empty());
    let next = run(&mut req, &db, &map);
    assert!(matches!(next[0].payload, ThumbMsgPayload::SourcePixels(_)));
    assert!(next[1].is_finalized());
    assert_eq!(
        db.load_one("page.png").unwrap().unwrap().source_dims,
        Some((16, 8))
    );
}

#[test]
fn virtual_parent_raw_continuation_saves_before_finalized_and_cache_hit_never_copies() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("book.zip");
    std::fs::write(&source, b"RAW continuation already owns decoded pixels").unwrap();
    let cache = temp.path().join("cache");
    let db = Arc::new(crate::catalog::CatalogDb::open(&cache, &source).unwrap());
    let parent = Arc::new(crate::catalog::CatalogDb::open(&cache, temp.path()).unwrap());
    let metadata = std::fs::metadata(&source).unwrap();
    let stamp = (
        crate::ui_helpers::mtime_secs(&metadata),
        metadata.len() as i64,
    );
    let mut req = LoadRequest {
        path: source.clone(),
        zip_entry: Some("first.cr2".into()),
        source_policy: LoadSourcePolicy::SourceOnly,
        mtime: stamp.0,
        file_size: stamp.1,
        raw_source: LoadRequestSource::RawHalfDeveloped {
            image: image::DynamicImage::new_rgb8(8, 12),
            developed_dims: [16, 24],
            decode_ms: 40.0,
            folder_selection_proof: None,
        },
        virtual_folder_parent: Some(Box::new(VirtualFolderParent {
            source_path: source,
            parent_catalog: parent.clone(),
            parent_key: "zipthumb:book.zip".into(),
            target_key: "first.cr2".into(),
            target_idx: 0,
            items_gen: 0,
            parent_entry_mtime: stamp.0,
            parent_entry_size: stamp.1,
        })),
        ..Default::default()
    };
    let map = RwLock::new(HashMap::new());
    let messages = run(&mut req, &db, &map);
    assert!(messages.last().unwrap().is_finalized());
    let copied = parent.load_one("zipthumb:book.zip").unwrap().unwrap();
    assert_eq!((copied.mtime, copied.file_size), stamp);
    assert_eq!(copied.source_dims, Some((16, 24)));
    req.raw_source = LoadRequestSource::RawHalfDeveloped {
        image: image::DynamicImage::new_rgb8(12, 8),
        developed_dims: [24, 16],
        decode_ms: 40.0,
        folder_selection_proof: None,
    };
    let repeated = run(&mut req, &db, &map);
    assert!(repeated.last().unwrap().is_finalized());
    assert_eq!(
        parent
            .load_one("zipthumb:book.zip")
            .unwrap()
            .unwrap()
            .source_dims,
        Some((24, 16)),
        "each source generation updates the parent, without a one-shot latch"
    );
    parent.delete_one("zipthumb:book.zip").unwrap();
    // The exact same correspondence on a DB hit must never publish a parent write.
    req.source_policy = LoadSourcePolicy::CacheOrSource;
    let messages = run(&mut req, &db, &RwLock::new(HashMap::new()));
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].pixels().unwrap().origin,
        ThumbLoadOrigin::UpgradeableCache
    );
    assert!(parent.load_one("zipthumb:book.zip").unwrap().is_none());
}

#[test]
fn virtual_parent_omitted_save_copies_only_existing_request_stamp_row() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("book.zip");
    std::fs::write(&source, b"RAW page source").unwrap();
    let cache = temp.path().join("cache");
    let db = Arc::new(crate::catalog::CatalogDb::open(&cache, &source).unwrap());
    let parent = Arc::new(crate::catalog::CatalogDb::open(&cache, temp.path()).unwrap());
    let metadata = std::fs::metadata(&source).unwrap();
    let stamp = (
        crate::ui_helpers::mtime_secs(&metadata),
        metadata.len() as i64,
    );
    let bytes = crate::catalog::encode_thumb_webp(&image::DynamicImage::new_rgb8(8, 12), 64, 75.0)
        .unwrap()
        .0;
    for row_stamp in [Some((7, 9)), Some((6, 9)), None] {
        parent.delete_one("zipthumb:book.zip").unwrap();
        db.delete_one("first.cr2").unwrap();
        if let Some((mtime, size)) = row_stamp {
            db.save_thumb_bytes(
                "first.cr2",
                mtime,
                size,
                Some((8, 12)),
                &bytes,
                &CoverIdentity::Unknown,
                crate::cover_auto_material::AutoMaterial::NotAutomatic,
            )
            .unwrap();
        }
        let mut req = LoadRequest {
            path: source.clone(),
            zip_entry: Some("first.cr2".into()),
            mtime: 7,
            file_size: 9,
            source_policy: LoadSourcePolicy::SourceOnly,
            raw_source: LoadRequestSource::RawHalfDeveloped {
                image: image::DynamicImage::new_rgb8(16, 24),
                developed_dims: [16, 24],
                decode_ms: 40.0,
                folder_selection_proof: None,
            },
            virtual_folder_parent: Some(Box::new(VirtualFolderParent {
                source_path: source.clone(),
                parent_catalog: parent.clone(),
                parent_key: "zipthumb:book.zip".into(),
                target_key: "first.cr2".into(),
                target_idx: 0,
                items_gen: 0,
                parent_entry_mtime: stamp.0,
                parent_entry_size: stamp.1,
            })),
            ..Default::default()
        };
        let map = RwLock::new(HashMap::new());
        let messages = run_with_policy(&mut req, &db, &map, crate::settings::CachePolicy::Off);
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::SourcePixels(_)
        ));
        assert!(messages.last().unwrap().is_finalized());
        assert!(map.read().unwrap().is_empty());
        let copied = parent.load_one("zipthumb:book.zip").unwrap();
        assert_eq!(copied.is_some(), row_stamp == Some((7, 9)));
        if let Some(copied) = copied {
            assert_eq!(copied.jpeg_data, bytes);
            assert_eq!(copied.source_dims, Some((8, 12)));
            assert_eq!((copied.mtime, copied.file_size), stamp);
        }
    }
}
