use super::*;

pub(super) fn source_stamp(path: &Path) -> (i64, i64) {
    let metadata = std::fs::metadata(path).unwrap();
    (
        crate::ui_helpers::mtime_secs(&metadata),
        metadata.len() as i64,
    )
}

// Invoke the worker publication boundary with a request admitted by the real
// pool attachment helper. No UI completion mutates or consumes the binding.
pub(super) fn writeback(
    app: &App,
    idx: usize,
    catalog: &crate::catalog::CatalogDb,
    stamp: (i64, i64),
) -> bool {
    let parent = app.virtual_folder_parent.as_ref().unwrap();
    let mut request = LoadRequest {
        path: parent.source_path.clone(),
        idx,
        mtime: stamp.0,
        file_size: stamp.1,
        cache_key_override: Some(parent.target_key.clone()),
        ..Default::default()
    };
    app.attach_pool_thumbnail_context(&mut request);
    if let Some(parent) = request.virtual_folder_parent {
        parent.writeback(catalog, stamp, request.input_seq);
        true
    } else {
        false
    }
}

fn webp(red: u8) -> Vec<u8> {
    crate::catalog::encode_thumb_webp(
        &image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            12,
            image::Rgba([red, 60, 200, 255]),
        )),
        32,
        80.0,
    )
    .unwrap()
    .0
}

#[test]
fn virtual_parent_worker_uses_persisted_row_and_retains_binding_for_regeneration() {
    let mut app = setup_app_for_test();
    let source = app.tmp.path().join("book.pdf");
    std::fs::write(&source, b"source stamp").unwrap();
    let cache_dir = app.tmp.path().join("catalogs");
    let parent_db = Arc::new(crate::catalog::CatalogDb::open(&cache_dir, app.tmp.path()).unwrap());
    let page_db = crate::catalog::CatalogDb::open(&cache_dir, &source).unwrap();
    let key = crate::grid_item::pdf_page_cache_key(0);
    let stamp = source_stamp(&source);
    app.virtual_folder_parent = Some(Box::new(VirtualFolderParent {
        source_path: source.clone(),
        parent_catalog: parent_db.clone(),
        parent_key: "pdfthumb:book.pdf".into(),
        target_key: key.clone(),
        target_idx: 3,
        items_gen: app.items_generation,
        parent_entry_mtime: stamp.0,
        parent_entry_size: stamp.1,
    }));
    for red in [20, 240] {
        let bytes = webp(red);
        page_db
            .save_thumb_bytes(
                &key,
                999,
                4096,
                Some((400, 600)),
                &bytes,
                &crate::cover_identity::CoverIdentity::Legacy,
                crate::cover_auto_material::AutoMaterial::NotAutomatic,
            )
            .unwrap();
        assert!(!writeback(&app, 2, &page_db, (999, 4096)));
        assert!(writeback(&app, 3, &page_db, (999, 4096)));
        let copied = parent_db.load_one("pdfthumb:book.pdf").unwrap().unwrap();
        assert_eq!((copied.mtime, copied.file_size), stamp);
        assert_eq!(copied.source_dims, Some((400, 600)));
        assert_eq!(copied.jpeg_data, bytes);
        assert!(app.virtual_folder_parent.is_some());
    }
    // A regenerated page from the formerly opened source cannot label a changed
    // document with the captured opening stamp.
    parent_db.delete_one("pdfthumb:book.pdf").unwrap();
    std::fs::write(&source, b"changed source with a different length").unwrap();
    assert!(writeback(&app, 3, &page_db, (999, 4096)));
    assert!(parent_db.load_one("pdfthumb:book.pdf").unwrap().is_none());
}

#[test]
fn virtual_parent_worker_skips_missing_or_mismatched_page_without_consuming_binding() {
    let mut app = setup_app_for_test();
    let source = app.tmp.path().join("book.pdf");
    std::fs::write(&source, b"source").unwrap();
    let cache_dir = app.tmp.path().join("catalogs");
    let parent_db = Arc::new(crate::catalog::CatalogDb::open(&cache_dir, app.tmp.path()).unwrap());
    let page_db = crate::catalog::CatalogDb::open(&cache_dir, &source).unwrap();
    let key = crate::grid_item::pdf_page_cache_key(0);
    let stamp = source_stamp(&source);
    app.virtual_folder_parent = Some(Box::new(VirtualFolderParent {
        source_path: source,
        parent_catalog: parent_db.clone(),
        parent_key: "pdfthumb:book.pdf".into(),
        target_key: key.clone(),
        target_idx: 0,
        items_gen: app.items_generation,
        parent_entry_mtime: stamp.0,
        parent_entry_size: stamp.1,
    }));
    assert!(writeback(&app, 0, &page_db, (9, 10)));
    assert!(parent_db.load_one("pdfthumb:book.pdf").unwrap().is_none());
    page_db
        .save_thumb_bytes(
            &key,
            1,
            2,
            None,
            &webp(80),
            &crate::cover_identity::CoverIdentity::Legacy,
            crate::cover_auto_material::AutoMaterial::NotAutomatic,
        )
        .unwrap();
    assert!(writeback(&app, 0, &page_db, (9, 10)));
    assert!(parent_db.load_one("pdfthumb:book.pdf").unwrap().is_none());
    assert!(app.virtual_folder_parent.is_some());
}

#[test]
fn virtual_parent_attachment_requires_source_key_index_and_generation_and_excludes_independent_readers()
 {
    let mut app = setup_app_for_test();
    let source = app.tmp.path().join("book.pdf");
    let parent_db = Arc::new(
        crate::catalog::CatalogDb::open(&app.tmp.path().join("catalogs"), app.tmp.path()).unwrap(),
    );
    let key = crate::grid_item::pdf_page_cache_key(0);
    app.virtual_folder_parent = Some(Box::new(VirtualFolderParent {
        source_path: source.clone(),
        parent_catalog: parent_db,
        parent_key: "pdfthumb:book.pdf".into(),
        target_key: key.clone(),
        target_idx: 0,
        items_gen: app.items_generation,
        parent_entry_mtime: 1,
        parent_entry_size: 2,
    }));
    let base = LoadRequest {
        path: source,
        cache_key_override: Some(key),
        ..Default::default()
    };
    let mut matching = base.clone();
    app.attach_pool_thumbnail_context(&mut matching);
    assert!(matching.virtual_folder_parent.is_some());
    for mismatch in ["source", "key", "index", "generation"] {
        let mut request = base.clone();
        match mismatch {
            "source" => request.path = app.tmp.path().join("other.pdf"),
            "key" => request.cache_key_override = Some(crate::grid_item::pdf_page_cache_key(1)),
            "index" => request.idx = 1,
            _ => {
                app.virtual_folder_parent.as_mut().unwrap().items_gen =
                    app.items_generation.wrapping_add(1)
            }
        }
        app.attach_pool_thumbnail_context(&mut request);
        assert!(request.virtual_folder_parent.is_none(), "{mismatch}");
    }
    app.virtual_folder_parent.as_mut().unwrap().items_gen = app.items_generation;
    for _reader in ["color", "preview"] {
        let mut request = base.clone();
        app.attach_viewer_thumbnail_context(&mut request);
        assert!(request.virtual_folder_parent.is_none());
    }
}

#[test]
fn virtual_seed_db_only_row_does_not_reseed_or_fill_sparse_map() {
    let mut app = setup_app_for_test();
    app.settings.cache_book_cover_only = false;
    let source = app.tmp.path().join("book.pdf");
    std::fs::write(&source, b"source").unwrap();
    let cache_dir = crate::catalog::default_cache_dir();
    let parent = crate::catalog::CatalogDb::open(&cache_dir, app.tmp.path()).unwrap();
    let target = crate::catalog::CatalogDb::open(&cache_dir, &source).unwrap();
    let key = crate::grid_item::pdf_page_cache_key(0);
    let retained = webp(20);
    target
        .save_thumb_bytes(
            &key,
            7,
            8,
            None,
            &retained,
            &crate::cover_identity::CoverIdentity::Legacy,
            crate::cover_auto_material::AutoMaterial::NotAutomatic,
        )
        .unwrap();
    parent
        .save_thumb_bytes(
            "pdfthumb:book.pdf",
            9,
            10,
            None,
            &webp(240),
            &crate::cover_identity::CoverIdentity::Legacy,
            crate::cover_auto_material::AutoMaterial::NotAutomatic,
        )
        .unwrap();
    app.items = vec![GridItem::PdfPage {
        pdf_path: source.clone(),
        page_num: 0,
        content_type: None,
    }];
    let map = Arc::new(std::sync::RwLock::new(HashMap::new()));
    app.setup_virtual_folder_seed_and_writeback(&source, &target, &map);
    assert_eq!(target.load_one(&key).unwrap().unwrap().jpeg_data, retained);
    assert!(map.read().unwrap().is_empty());
}

#[test]
fn video_seed_db_only_match_change_missing_and_corrupt() {
    use crate::folder_thumb_pins::{FileKind, FolderPinSource};
    let mut app = setup_app_for_test();
    let parent = app.tmp.path().join("video-seed");
    let folder = parent.join("book");
    std::fs::create_dir_all(&folder).unwrap();
    let video = folder.join("clip.mp4");
    std::fs::write(&video, b"video").unwrap();
    let source = FolderPinSource::File {
        rel: "clip.mp4".into(),
        kind: FileKind::Video,
    };
    app.folder_pin_map.insert(
        crate::path_key::normalize_keep_drive(&folder),
        source.clone().into(),
    );
    app.current_folder = Some(parent.clone());
    app.items = vec![GridItem::Folder(folder.clone())];
    let resolved = crate::folder_thumb_pins::resolve_pin_target(&folder, &source).unwrap();
    let base = crate::thumb_loader::folder_thumb_cache_key_for_path(
        &folder,
        app.use_full_path_cache_keys(),
        app.settings.folder_thumb_sort,
        app.settings.folder_thumb_depth,
        crate::catalog::FolderThumbProvenance::Seeded,
    )
    .unwrap();
    let key = resolved.pinned_cache_key(&base, crate::folder_thumb_pins::PinKeyMode::Exact, None);
    let catalog = Arc::new(
        crate::catalog::CatalogDb::open(&app.tmp.path().join("catalogs"), &parent).unwrap(),
    );
    let map = Arc::new(std::sync::RwLock::new(HashMap::new()));
    let cover =
        crate::cover_identity::CoverIdentity::Page(crate::adjustment_db::normalize_path(&video));
    for scenario in ["matching", "changed", "missing", "corrupt"] {
        let old = webp(20);
        catalog
            .save_seeded_folder_bytes(
                &key,
                resolved.mtime,
                resolved.file_size,
                None,
                &old,
                &cover,
                crate::cover_auto_material::AutoMaterial::NotAutomatic,
            )
            .unwrap();
        map.write().unwrap().clear();
        let bytes = match scenario {
            "matching" => old.clone(),
            "changed" => webp(240),
            "corrupt" => b"broken".to_vec(),
            _ => Vec::new(),
        };
        if scenario == "missing" {
            app.video_pin_db.as_ref().unwrap().remove(&video).unwrap();
        } else {
            app.video_pin_db
                .as_ref()
                .unwrap()
                .set_pin(&video, 1.0, &bytes)
                .unwrap();
        }
        let revision = catalog.revision_state();
        app.seed_folder_video_pin_thumbs(&map, Some(&catalog), None);
        match scenario {
            "matching" => {
                assert_eq!(catalog.revision_state(), revision);
                assert!(map.read().unwrap().is_empty());
            }
            "changed" => {
                assert_eq!(catalog.load_one(&key).unwrap().unwrap().jpeg_data, bytes);
                assert_eq!(map.read().unwrap()[&key].jpeg_data, bytes);
            }
            _ => {
                assert!(!catalog.contains_filename(&key).unwrap(), "{scenario}");
                assert!(map.read().unwrap().is_empty());
            }
        }
    }
}
