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
