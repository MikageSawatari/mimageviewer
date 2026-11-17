use super::*;
use crate::auto_aspect_cache::AutoAspectCacheEntry;
use crate::settings::ThumbAspect;

fn aspect_entry(
    aspect: ThumbAspect,
    sample_count: usize,
    eligible_total: usize,
) -> AutoAspectCacheEntry {
    AutoAspectCacheEntry {
        aspect,
        sample_count,
        eligible_total,
        updated_at: 0,
    }
}

#[test]
fn reset_restores_saved_geometry_before_rendering_without_collecting_samples() {
    let mut app = setup_app_for_test();
    app.settings.thumb_aspect_auto = true;
    let folder = app.tmp.path().join("remembered");
    app.current_folder = Some(folder.clone());
    app.items = (0..40)
        .map(|idx| GridItem::Image(folder.join(format!("{idx}.png"))))
        .collect();
    app.image_metas = vec![Some((1, 2)); 40];
    app.items_generation = 17;
    app.auto_aspect.samples.insert(0, 1.0);
    app.auto_aspect.switches_done = 2;
    app.auto_aspect.streak = Some((ThumbAspect::Square, Instant::now(), 1));
    app.last_cell_size = 100.0;
    app.auto_aspect_cache_db
        .as_ref()
        .unwrap()
        .upsert(&folder, ThumbAspect::Portrait2x3, 24, 40)
        .unwrap();

    app.reset_auto_aspect_with_seed(None);

    assert_eq!(app.auto_aspect.items_generation, 17);
    assert_eq!(app.effective_thumb_aspect(), ThumbAspect::Portrait2x3);
    assert_eq!(app.last_cell_h, 150.0);
    assert_eq!(app.auto_aspect.cached_sample_gate, Some(24));
    assert!(app.auto_aspect.samples.is_empty());
    assert_eq!(app.auto_aspect.switches_done, 0);
    assert!(app.auto_aspect.streak.is_none());
    assert!(app.auto_aspect.last_switch_at.is_none());

    app.settings.thumb_aspect_auto = false;
    app.settings.thumb_aspect = ThumbAspect::Landscape16x9;
    app.reset_auto_aspect_with_seed(Some(aspect_entry(ThumbAspect::Portrait9x16, 8, 8)));
    assert!(app.auto_aspect.current.is_none());
    assert!(app.auto_aspect.cached_sample_gate.is_none());
    assert_eq!(app.effective_thumb_aspect(), ThumbAspect::Landscape16x9);
}

#[test]
fn ordinary_resume_skips_initial_samples_even_when_virtual_seed_populates_map() {
    let mut app = setup_app_for_test();
    app.settings.thumb_aspect_auto = true;
    app.settings.cache_book_cover_only = false;
    let folder = app.tmp.path().join("ordinary-parent");
    std::fs::create_dir_all(&folder).unwrap();
    let source = folder.join("book.zip");
    std::fs::write(&source, b"source stamp fixture").unwrap();
    let cache_dir = crate::catalog::default_cache_dir();
    let parent = crate::catalog::CatalogDb::open(&cache_dir, &folder).unwrap();
    let pixels =
        crate::catalog::encode_thumb_webp(&image::DynamicImage::new_rgb8(80, 120), 32, 80.0)
            .unwrap()
            .0;
    parent
        .save_thumb_bytes(
            "zipthumb:book.zip",
            1,
            2,
            Some((80, 120)),
            &pixels,
            &crate::cover_identity::CoverIdentity::Legacy,
            crate::cover_auto_material::AutoMaterial::NotAutomatic,
        )
        .unwrap();

    app.start_loading_items(
        source.clone(),
        vec![GridItem::ZipImage {
            zip_path: source,
            entry_name: "cover.png".into(),
        }],
        vec![Some((1, 2))],
        HashSet::from(["cover.png".into()]),
        Vec::new(),
        None,
    );
    app.finish_page_identity_preparation_for_test();

    let map = app.current_color_cache_map.as_ref().unwrap().clone();
    assert!(
        map.read().unwrap().contains_key("cover.png"),
        "the existing virtual seed remains available"
    );
    assert!(
        app.auto_aspect.samples.is_empty(),
        "ordinary resume must not probe even a nonempty map"
    );
    assert_eq!(app.effective_thumb_aspect(), ThumbAspect::Square);
    // The prepared-map wrapper still performs its original initial sampling.
    app.reset_and_seed_auto_aspect(&map);
    assert_eq!(app.auto_aspect.samples.len(), 1);
    assert_eq!(app.effective_thumb_aspect(), ThumbAspect::Portrait2x3);
}

#[test]
fn prepared_aggregate_and_collection_wrappers_keep_initial_sampling() {
    let mut app = setup_app_for_test();
    app.settings.thumb_aspect_auto = true;
    let item = GridItem::Image(app.tmp.path().join("sample.png"));
    app.items = vec![item];
    app.image_metas = vec![Some((1, 2))];
    let map = Arc::new(std::sync::RwLock::new(HashMap::from([(
        "sample.png".into(),
        crate::catalog::CacheEntry {
            mtime: 1,
            file_size: 2,
            jpeg_data: Vec::new(),
            source_dims: Some((80, 120)),
            layout_dims: None,
            cover: crate::cover_identity::CoverIdentity::Legacy,
            auto_material: crate::cover_auto_material::AutoMaterial::NotAutomatic,
            folder_provenance: None,
            selection_proof: None,
        },
    )])));
    for prepared_total in [None, Some(1)] {
        app.reset_and_seed_auto_aspect_with_seed(&map, None, prepared_total);
        assert_eq!(app.auto_aspect.samples.get(&0), Some(&1.5));
        assert_eq!(app.effective_thumb_aspect(), ThumbAspect::Portrait2x3);
    }
}

#[test]
fn rating_install_preserves_legacy_synthetic_pixels_and_initial_samples_without_parent_db() {
    use crate::rating_db::{RatingItemKind, RatingMeta};
    use crate::thumb_loader::{LoadSourcePolicy, RequestCatalog, ThumbMsgPayload};

    let mut app = setup_app_for_test();
    app.settings.thumb_aspect_auto = true;
    app.settings.cache_policy = crate::settings::CachePolicy::Off;
    app.settings.rating_view_sort =
        crate::rating_view::RatingViewSort::Normal(crate::settings::SortOrder::FileName);
    let folder = app.tmp.path().join("rating-legacy-only-parent");
    std::fs::create_dir(&folder).unwrap();
    let cache_dir = crate::catalog::default_cache_dir();
    let parent_db_path = crate::catalog::db_path_for(&cache_dir, &folder);
    assert!(!parent_db_path.exists());
    let synthetic = crate::app::rating_view_synthetic_path();
    let old_catalog = crate::catalog::CatalogDb::open(&cache_dir, &synthetic).unwrap();
    let bytes = crate::catalog::encode_thumb_webp(
        &image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            80,
            120,
            image::Rgb([200, 60, 20]),
        )),
        32,
        80.0,
    )
    .unwrap()
    .0;
    let expected_pixels = crate::catalog::decode_thumb_to_color_image(&bytes).unwrap();
    for index in 0..8 {
        let path = folder.join(format!("rated-{index}.png"));
        // The real Rating builder can stat these rows, but source decoding cannot
        // accidentally make this legacy-cache preservation test pass.
        std::fs::write(&path, b"intentionally not a decodable PNG").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let page_key = crate::adjustment_db::normalize_path(&path);
        app.rating_db
            .as_ref()
            .unwrap()
            .set_user_rating(
                &page_key,
                3,
                Some(&RatingMeta::new(RatingItemKind::Image).with_source_path(&path)),
            )
            .unwrap();
        assert!(
            old_catalog
                .save_thumb_bytes(
                    &crate::app::image_full_path_cache_key(&path),
                    crate::ui_helpers::mtime_secs(&metadata),
                    metadata.len() as i64,
                    Some((80, 120)),
                    &bytes,
                    &crate::cover_identity::CoverIdentity::Page(page_key),
                    crate::cover_auto_material::AutoMaterial::NotAutomatic,
                )
                .unwrap()
        );
    }
    drop(old_catalog);

    // Exercise the production Rating admission/build/install/hydration route,
    // not reset_and_seed_auto_aspect with a manually supplied cache map.
    app.current_folder = Some(folder);
    app.enter_rating_view(3);
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.poll_rating_view();
        app.poll_sidecar_restore(&ctx);
        if app.rating_view_pending.is_none() && !app.sidecar_restore_active() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "real Rating install did not settle"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(app.items_are_rating_view);
    assert_eq!(app.current_folder.as_deref(), Some(synthetic.as_path()));
    assert_eq!(app.items.len(), 8);
    assert_eq!(
        app.auto_aspect.samples.len(),
        crate::auto_aspect::min_samples_for(8)
    );
    assert_eq!(app.effective_thumb_aspect(), ThumbAspect::Portrait2x3);
    let map = app.current_color_cache_map.as_ref().unwrap().clone();
    assert_eq!(
        map.read().unwrap().len(),
        8,
        "synthetic install must load the existing legacy rows"
    );
    assert!(!parent_db_path.exists());

    let (mtime, file_size) = app.image_metas[0].unwrap();
    let mut request = crate::app::make_load_request(
        &app.items[0],
        0,
        mtime,
        file_size,
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
    )
    .unwrap();
    request.source_policy = LoadSourcePolicy::CacheOnly;
    app.attach_pool_thumbnail_context(&mut request);
    assert!(matches!(
        &request.catalog_target,
        RequestCatalog::Parent { .. }
    ));
    let key = crate::thumb_loader::cache_key_for_request(&request)
        .unwrap()
        .into_owned();
    assert_eq!(map.read().unwrap().get(&key).unwrap().jpeg_data, bytes);
    app.keep_start_shared.store(0, Ordering::Relaxed);
    app.keep_end_shared.store(8, Ordering::Relaxed);
    let queue = app.reload_queue.as_ref().unwrap().clone();
    queue.0.lock().unwrap().push(request.clone());
    queue.1.notify_one();
    let message = app
        .rx
        .recv_timeout(Duration::from_secs(5))
        .expect("real worker cache hit");
    let pixels = message
        .pixels()
        .expect("legacy memory row must provide cache pixels");
    assert!(pixels.origin.from_cache());
    assert_eq!(pixels.image.size, expected_pixels.size);
    assert_eq!(pixels.image.pixels, expected_pixels.pixels);
    assert!(
        !parent_db_path.exists(),
        "memory hit must not create a Parent catalog"
    );

    // A negative control proves the Parent fallback has no pixels to return.
    // The first successful result therefore came from the installed legacy map.
    map.write().unwrap().clear();
    queue.0.lock().unwrap().push(request);
    queue.1.notify_one();
    let miss = app
        .rx
        .recv_timeout(Duration::from_secs(5))
        .expect("real worker cache-only miss");
    assert!(matches!(miss.payload, ThumbMsgPayload::CacheOnlyMiss));
    assert!(!parent_db_path.exists());
}

#[test]
fn arriving_samples_keep_saved_sample_input_idle_and_streak_gates() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.settings.thumb_aspect_auto = true;
    app.items = (0..40)
        .map(|idx| GridItem::Image(app.tmp.path().join(format!("{idx}.png"))))
        .collect();
    app.image_metas = vec![Some((1, 2)); 40];
    app.thumbnails = vec![ThumbnailState::Pending; 40];
    app.keep_set.extend(0..40);
    app.keep_range = (0, 40);
    app.reset_auto_aspect_with_seed(Some(aspect_entry(ThumbAspect::Square, 12, 40)));
    app.last_input_at = Some(Instant::now());
    let receive = |app: &mut App, range: std::ops::Range<usize>| {
        for idx in range {
            app.requested.insert(idx, false);
            send_thumbnail_for_test!(
                app,
                crate::thumb_loader::ThumbMsg::from_legacy_parts(
                    idx,
                    Some(egui::ColorImage::filled([2, 3], egui::Color32::WHITE)),
                    crate::thumb_loader::ThumbLoadOrigin::FinalCache,
                    false,
                    None,
                    Some((80, 120)),
                    None,
                    false,
                    false,
                    0,
                    app.items_generation,
                )
            )
            .unwrap();
        }
        for _ in 0..40 {
            app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
            if app.requested.images_len() == 0 {
                break;
            }
        }
        assert_eq!(app.requested.images_len(), 0);
    };
    receive(&mut app, 0..10);
    app.last_input_at = None;
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert_eq!(app.auto_aspect.samples.len(), 10);
    assert_eq!(app.auto_aspect.cached_sample_gate, Some(12));
    assert_eq!(app.effective_thumb_aspect(), ThumbAspect::Square);
    assert!(app.auto_aspect.streak.is_none());

    app.last_input_at = Some(Instant::now());
    receive(&mut app, 10..12);
    assert_eq!(app.auto_aspect.samples.len(), 12);
    assert_eq!(
        app.auto_aspect.cached_sample_gate,
        Some(12),
        "input idle gate still runs first"
    );
    assert_eq!(app.effective_thumb_aspect(), ThumbAspect::Square);
    app.last_input_at = None;
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(app.auto_aspect.cached_sample_gate.is_none());
    assert_eq!(
        app.effective_thumb_aspect(),
        ThumbAspect::Square,
        "arrival does not bypass the streak"
    );
    let (candidate, _, samples) = app.auto_aspect.streak.unwrap();
    app.auto_aspect.streak = Some((candidate, Instant::now() - Duration::from_secs(1), samples));
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert_eq!(app.effective_thumb_aspect(), ThumbAspect::Portrait2x3);
    assert_eq!(app.auto_aspect.switches_done, 1);
}
