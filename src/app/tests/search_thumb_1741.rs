use super::phase_c_support::setup_app;
use super::*;
use crate::thumb_loader::{RequestCatalog, ThumbMsgPayload, ThumbnailCatalogProvider};
use std::io::Write;

fn zip_fixture(path: &Path) {
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(16, 12)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    writer
        .start_file("cover.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(png.get_ref()).unwrap();
    writer.finish().unwrap();
}

fn request(app: &App, item: &GridItem, stamp: (i64, i64)) -> Option<LoadRequest> {
    make_load_request_with_visibility(
        item,
        0,
        stamp.0,
        stamp.1,
        false,
        None,
        Some(app.settings.folder_thumb_sort),
        app.settings.folder_thumb_depth,
        &app.folder_pin_map,
        &app.converted_archive_cache_paths,
        None,
        app.current_folder.as_deref(),
        app.folder_thumb_pin_db.as_deref(),
        app.video_pin_db.as_ref(),
        app.use_full_path_cache_keys(),
        &app.settings.file_type_visibility,
    )
}

fn process(app: &App, req: &mut LoadRequest) -> Vec<crate::thumb_loader::ThumbMsg> {
    let (tx, rx) = mpsc::channel();
    crate::thumb_loader::process_load_request(
        req,
        app.current_color_cache_map.as_ref().unwrap(),
        &tx,
        ThumbnailCatalogProvider::fixed(app.current_color_catalog.clone()),
        64,
        75,
        64,
        CacheDecision::from_settings(&app.settings),
        &Arc::new(AtomicUsize::new(0)),
        &Arc::new(Mutex::new(crate::stats::ThumbStats::default())),
        None,
        &Arc::new(AtomicUsize::new(0)),
        &Arc::new(AtomicUsize::new(1)),
        None,
        None,
        None,
        None,
        None,
    );
    rx.try_iter().collect()
}

#[test]
fn section1741_everything_zip_generation_reuses_parent_catalog_and_folder_selection() {
    let mut env = setup_app();
    let origin = env.tmp.path().join("A");
    let parent = env.tmp.path().join("root/F01");
    std::fs::create_dir(&origin).unwrap();
    std::fs::create_dir_all(&parent).unwrap();
    let zip = parent.join("vol01.zip");
    zip_fixture(&zip);
    env.load_folder(origin.clone());
    env.finish_page_identity_preparation_for_test();
    env.settings.cache_policy = crate::settings::CachePolicy::Always;
    env.settings.thumb_idle_upgrade = false;
    let original_db = env.current_color_catalog.clone().unwrap();
    let original_memory = env.current_color_cache_map.clone().unwrap();
    let original_token = env.cancel_token.clone();
    crate::global_search_ui::adopt_everything_for_test(
        &mut env,
        &egui::Context::default(),
        "vol01",
        crate::global_search_ui::everything_dataset_for_test(vec![GridItem::ZipFile(zip.clone())]),
    );
    assert!(original_token.load(Ordering::Relaxed));
    assert_eq!(
        env.thumbnail_pool_catalog,
        ThumbnailPoolCatalog::View(search_results_synthetic_path())
    );
    let item = env.items[0].clone();
    // The fake producer supplies its own list stamp; the parent must use observed attributes.
    let request_stamp = env.image_metas[0].unwrap();
    let mut req = request(&env, &item, request_stamp).unwrap();
    env.attach_viewer_thumbnail_context(&mut req);
    let RequestCatalog::Parent { row, .. } = &req.catalog_target else {
        panic!("Search cover admission must select its real parent");
    };
    assert_eq!(row.folder, parent);
    assert_eq!(row.row_key, "zipthumb:vol01.zip");
    let outer_key = req.cache_key_override.clone().unwrap();
    assert!(
        process(&env, &mut req)
            .iter()
            .any(|m| matches!(m.payload, ThumbMsgPayload::SourcePixels(_)))
    );
    let parent_db =
        crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), &parent).unwrap();
    let row = parent_db.load_one("zipthumb:vol01.zip").unwrap().unwrap();
    let observed = crate::catalog::file_stamp(&std::fs::metadata(&zip).unwrap());
    assert_eq!((row.mtime, row.file_size), observed);
    assert!(original_db.load_all().unwrap().is_empty());
    assert!(original_memory.read().unwrap().is_empty());
    assert!(
        env.current_color_catalog
            .as_ref()
            .unwrap()
            .load_all()
            .unwrap()
            .is_empty()
    );
    let memory = env
        .current_color_cache_map
        .as_ref()
        .unwrap()
        .read()
        .unwrap();
    assert_eq!(
        (memory[&outer_key].mtime, memory[&outer_key].file_size),
        request_stamp
    );
    assert!(!memory.contains_key("zipthumb:vol01.zip"));
    drop(memory);
    assert_eq!(
        crate::thumb_loader::parent_cached_selection_for_test(
            &parent,
            &crate::catalog::default_cache_dir()
        ),
        Some("zipthumb:vol01.zip".into())
    );
    // Reopening the physical parent uses exactly the generated row.
    env.close_global_search();
    env.finish_history_restore_preparation_for_test();
    env.load_folder(parent.clone());
    env.finish_page_identity_preparation_for_test();
    let mut normal = request(&env, &GridItem::ZipFile(zip), observed).unwrap();
    env.attach_viewer_thumbnail_context(&mut normal);
    assert!(matches!(
        normal.catalog_target,
        RequestCatalog::Fixed { .. }
    ));
    assert!(
        process(&env, &mut normal)
            .iter()
            .any(|m| matches!(m.payload, ThumbMsgPayload::Pixels(ref p) if p.origin.from_cache()))
    );
}

#[test]
fn section1741_parent_rows_are_only_unpinned_single_file_cover_branches() {
    let mut env = setup_app();
    let root = env.tmp.path().to_path_buf();
    let zip = root.join("book.zip");
    let pdf = root.join("book.pdf");
    let epub = root.join("book.epub");
    let image = root.join("cover.png");
    zip_fixture(&zip);
    for path in [&pdf, &epub, &image] {
        std::fs::write(path, b"fixture").unwrap();
    }
    env.items_are_global_search_view = true;
    let eligible = [
        GridItem::Image(image.clone()),
        GridItem::ZipFile(zip.clone()),
        GridItem::PdfFile(pdf.clone()),
        GridItem::PdfFile(epub.clone()),
    ];
    for item in eligible {
        let mut req = request(&env, &item, (1, 0)).unwrap();
        let candidate = req.parent_row.as_ref().unwrap();
        assert_eq!(candidate.folder, root);
        if matches!(item, GridItem::PdfFile(_)) {
            assert_eq!(req.pdf_page, Some(0));
        }
        env.attach_viewer_thumbnail_context(&mut req);
        assert!(matches!(req.catalog_target, RequestCatalog::Parent { .. }));
    }
    let archive = root.join("book.7z");
    env.converted_archive_cache_paths.insert(
        crate::path_key::normalize_keep_drive(&archive),
        ConvertedArchiveSourceState::Direct(zip.clone()),
    );
    let excluded = [
        GridItem::Folder(root.clone()),
        GridItem::ConvertibleArchive {
            path: archive,
            format: crate::archive_converter::ArchiveFormat::SevenZ,
        },
        GridItem::Stack {
            key: "cover".into(),
            representative: image.clone(),
            count: 2,
        },
        GridItem::ZipImage {
            zip_path: zip.clone(),
            entry_name: "cover.png".into(),
        },
        GridItem::PdfPage {
            pdf_path: pdf.clone(),
            page_num: 0,
            content_type: None,
        },
        GridItem::ZipDir {
            zip_path: zip.clone(),
            dir_prefix: "chapter/".into(),
            is_archive: false,
            representative: Some("cover.png".into()),
        },
        GridItem::SearchContainer {
            path: root.clone(),
            kind: crate::grid_item::SearchContainerKind::Folder,
            hit_count: 1,
            representative: Some(crate::grid_item::ContainerRepresentative {
                path: image,
                zip_entry: None,
                pdf_page: None,
            }),
        },
        GridItem::Audio(root.join("music.mp3")),
        GridItem::Video(root.join("video.mp4")),
    ];
    for item in excluded {
        if let Some(mut req) = request(&env, &item, (1, 0)) {
            assert!(req.parent_row.is_none(), "{item:?}");
            env.attach_viewer_thumbnail_context(&mut req);
            assert!(
                matches!(req.catalog_target, RequestCatalog::Fixed { .. }),
                "{item:?}"
            );
        }
    }
    for (item, source) in [
        (
            GridItem::ZipFile(zip.clone()),
            crate::folder_thumb_pins::FolderPinSource::ZipEntry {
                zip_rel: String::new(),
                entry: "cover.png".into(),
            },
        ),
        (
            GridItem::PdfFile(pdf.clone()),
            crate::folder_thumb_pins::FolderPinSource::PdfPage {
                pdf_rel: String::new(),
                page: 1,
            },
        ),
        (
            GridItem::PdfFile(epub.clone()),
            crate::folder_thumb_pins::FolderPinSource::PdfPage {
                pdf_rel: String::new(),
                page: 1,
            },
        ),
    ] {
        env.folder_pin_map.insert(
            crate::path_key::normalize_keep_drive(item.drag_source_path().unwrap()),
            source,
        );
        let mut req = request(&env, &item, (1, 0)).unwrap();
        assert!(
            req.cache_key_override
                .as_ref()
                .unwrap()
                .contains(crate::thumb_loader::CACHE_KEY_PIN_SUFFIX)
        );
        assert!(req.parent_row.is_none());
        env.attach_viewer_thumbnail_context(&mut req);
        assert!(matches!(req.catalog_target, RequestCatalog::Fixed { .. }));
    }
}

#[test]
fn section1741_storage_scope_and_root_keys_preserve_single_folder_contract() {
    let mut env = setup_app();
    env.current_folder = Some(env.tmp.path().join("physical"));
    assert_eq!(
        env.thumb_storage_scope(),
        ThumbStorageScope::SingleFolder {
            full_path_keys: false
        }
    );
    for path in [
        search_results_synthetic_path(),
        rating_view_synthetic_path(),
        bookmark_view_synthetic_path(),
        reading_history_synthetic_path(),
        subfolder_expansion_synthetic_path(),
        smart_folder::smart_folder_synthetic_path(uuid::Uuid::new_v4()),
    ] {
        env.current_folder = Some(path);
        assert_eq!(env.thumb_storage_scope(), ThumbStorageScope::MultiParent);
        assert!(env.use_full_path_cache_keys());
    }
    env.current_folder = Some(env.tmp.path().join("book.zip"));
    assert_eq!(
        env.thumb_storage_scope(),
        ThumbStorageScope::SingleFolder {
            full_path_keys: false
        }
    );
    for flag in 0..5 {
        env.items_are_global_search_view = flag == 0;
        env.items_are_tag_view = flag == 1;
        env.items_are_reading_history_view = flag == 2;
        env.items_are_rating_view = flag == 3;
        env.items_are_bookmark_view = flag == 4;
        assert_eq!(env.thumb_storage_scope(), ThumbStorageScope::MultiParent);
    }
    env.items_are_bookmark_view = false;
    env.items_are_drive_list = true;
    assert_eq!(env.thumb_storage_scope(), ThumbStorageScope::DriveList);
    env.items_are_global_search_view = true;
    assert_eq!(
        env.thumb_storage_scope(),
        ThumbStorageScope::MultiParent,
        "Search can retain the outgoing drive-list flag until its return load"
    );
    env.items_are_global_search_view = false;
    env.items_are_drive_list = false;
    env.current_folder = Some(PathBuf::from("C:\\"));
    assert_eq!(
        env.thumb_storage_scope(),
        ThumbStorageScope::SingleFolder {
            full_path_keys: true
        }
    );
    let root_image = GridItem::Image(PathBuf::from("C:\\cover.png"));
    let root_zip = GridItem::ZipFile(PathBuf::from("C:\\vol01.zip"));
    for (item, expected) in [
        (root_image, "imgthumb:C:\\cover.png"),
        (root_zip, "zipthumb:C:\\vol01.zip"),
    ] {
        let mut req = request(&env, &item, (1, 2)).unwrap();
        assert_eq!(req.parent_row.as_ref().unwrap().row_key, expected);
        env.attach_viewer_thumbnail_context(&mut req);
        assert!(matches!(req.catalog_target, RequestCatalog::Fixed { .. }));
    }
}
