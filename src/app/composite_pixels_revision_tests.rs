#![cfg(test)]

use super::paged_prefetch_tests::{complete_pending_composites, pdf_book, retain_pdf_ai};
use super::*;

pub(crate) fn warm_final_pair(ctx: &egui::Context) -> (AppTestEnvForTest, [FinalCompositeKey; 2]) {
    let mut app = pdf_book(ctx);
    let keys = [retain_pdf_ai(&mut app, 21), retain_pdf_ai(&mut app, 0)];
    for _ in 0..2 {
        app.prefetch_retained_paged_composites(ctx, 19);
        assert!(!app.final_effect_pending.is_empty());
        complete_pending_composites(&mut app, ctx);
    }
    for key in keys {
        assert!(app.final_composite_cache.get(&key).unwrap().complete);
        assert!(!app.fs_cache.contains_key(&key.edit_key.idx));
        assert!(!app.edit_result_cache.contains_key(&key.edit_key));
    }
    (app, keys)
}

pub(crate) fn evict_pair_cpu(app: &mut App) {
    app.settings.retained_final_ai_cache_max_entries = 1;
    retain_pdf_ai(app, 10);
    assert!(!app.has_retained_pdf_final_ai_for_current_params(0));
    assert!(!app.has_retained_pdf_final_ai_for_current_params(21));
    assert_eq!(app.retained_cross_session_cache_entries(), 1);
    // The completed composite is the sole remaining source of these pixels.
    app.final_ai_cache.clear();
}

#[test]
fn composite_pixels_revision_final_pair_navigation_capture_survives_retained_cpu_eviction() {
    let ctx = egui::Context::default();
    let (mut app, keys) = warm_final_pair(&ctx);
    evict_pair_cpu(&mut app);
    app.fullscreen_idx = Some(21);
    for key in keys {
        let expected = Arc::clone(&app.final_composite_cache.get(&key).unwrap().pixels);
        let (actual_key, actual) = app
            .ensure_final_composite_pixels_with_key(&ctx, key.edit_key.idx)
            .expect("complete live pixels must not require raw/edit pixels");
        assert_eq!(actual_key, key);
        assert!(Arc::ptr_eq(&actual, &expected));
    }
    // Supplement covers are displayed beside the tail, but output captures
    // navigation pages only. Both pages' real capture consumers still need pixels.
    for mode in [
        crate::settings::SpreadMode::LtrCover,
        crate::settings::SpreadMode::RtlCover,
    ] {
        app.spread_mode = mode;
        app.viewer_navigation_caches.invalidate();
        assert_eq!(app.displayed_spread_partner(21), Some(0));
        for idx in [21, 0] {
            let work = app.prepare_capture_pixel_work_for_test(&ctx, idx).unwrap();
            let crate::capture::CapturePixelWork::Single(job) = &work else {
                panic!("supplement display must preserve single navigation-page capture");
            };
            let (_, expected) = app
                .ensure_final_composite_pixels_with_key(&ctx, idx)
                .unwrap();
            assert!(Arc::ptr_eq(&job.source, &expected));
            assert_eq!(job.source.size, [8, 12]);
            let (_, width, height, rgba) = crate::capture::run_pixel_work(work).unwrap();
            assert_eq!((width, height), (8, 12));
            assert!(
                rgba.chunks_exact(4)
                    .all(|pixel| pixel == [255, 255, 255, 255])
            );
        }
    }
    for idx in [0, 21] {
        assert!(!app.fs_cache.contains_key(&idx));
        assert!(!app.fs_pending.contains_key(&idx));
        assert!(!app.fs_upload_backlog_contains(idx));
    }
    assert!(app.edit_result_cache.is_empty());
    assert!(app.final_ai_pending.is_empty());
    assert!(app.final_effect_pending.is_empty());
}

pub(crate) fn seed_annotation(app: &mut App, source_dims: [usize; 2]) {
    // A filled bubble needs no font or stamp I/O, but uses the real annotation compositor.
    app.comic_font_registry_loaded = true;
    app.comic_fonts_loaded = true;
    app.comic_fonts = Some(Arc::new(comic_core::FontSet::new()));
    let bubble = comic_core::BubbleObject {
        shape: comic_core::BubbleShape::RoundRect {
            half_w: source_dims[0] as f32 / 4.0,
            half_h: source_dims[1] as f32 / 4.0,
            corner_px: 0.0,
        },
        fill: Some(comic_core::Rgba::new(255, 0, 0, 255)),
        outline: comic_core::StrokeStyle {
            width_px: 0.0,
            ..Default::default()
        },
        auto_size: false,
        ..Default::default()
    };
    let page = app.page_path_key(0).unwrap();
    app.comic_docs.insert(
        page,
        vec![comic_core::AnnotationObject::new_bubble(
            1,
            (source_dims[0] as f32 / 2.0, source_dims[1] as f32 / 2.0),
            bubble,
        )],
    );
}

pub(crate) fn complete_comic(app: &mut App, ctx: &egui::Context) -> Arc<egui::ColorImage> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !app.comic_bake_pending.is_empty() {
        app.poll_comic_bake(ctx);
        assert!(
            std::time::Instant::now() < deadline,
            "comic worker did not finish"
        );
        std::thread::yield_now();
    }
    assert!(app.ensure_comic_composite_texture(ctx, 0).is_some());
    Arc::clone(&app.comic_cache.get(&0).unwrap().pixels)
}

#[test]
fn composite_pixels_revision_annotation_display_and_export_preserve_canonical_basis() {
    // The second case models a GPU-clamped canvas whose canonical source is larger.
    for source_dims in [[4, 6], [16_000, 24_000]] {
        let ctx = egui::Context::default();
        let (mut app, keys) = warm_final_pair(&ctx);
        let raw = Arc::new(egui::ColorImage::filled([4, 6], egui::Color32::WHITE));
        let tex = ctx.load_texture(
            "annotation-source",
            Arc::clone(&raw),
            DISPLAY_IMAGE_TEXTURE_OPTIONS,
        );
        app.fs_cache.insert(
            0,
            FsCacheEntry::Static {
                tex,
                pixels: raw,
                source_dims: Some(source_dims),
                load_seq: 1,
                animation: StaticAnimationState::Still,
            },
        );
        app.record_fs_cache_page_dims_for_spread(0);
        seed_annotation(&mut app, source_dims);
        let before_export = app.comic_composited_pixels_for_export(&ctx, 0).unwrap();
        assert!(app.ensure_comic_composite_texture(&ctx, 0).is_none());
        let before_display = complete_comic(&mut app, &ctx);
        assert_eq!(before_display.pixels, before_export.pixels);
        assert_eq!(before_export[(4, 6)], egui::Color32::RED);
        assert_eq!(before_export[(0, 0)], egui::Color32::WHITE);
        let before_capture = app.prepare_capture_pixel_work_for_test(&ctx, 0).unwrap();
        let crate::capture::CapturePixelWork::Single(job) = &before_capture else {
            panic!("cover capture must use its single navigation page");
        };
        assert_eq!(job.source.pixels, before_export.pixels);
        let (_, before_width, before_height, before_rgba) =
            crate::capture::run_pixel_work(before_capture).unwrap();

        app.fs_cache.remove(&0);
        evict_pair_cpu(&mut app);
        app.comic_cache.clear();
        assert_eq!(
            app.final_composite_cache.get(&keys[1]).unwrap().source_size,
            [4, 6]
        );
        let after_export = app.comic_composited_pixels_for_export(&ctx, 0).unwrap();
        assert_eq!(after_export.pixels, before_export.pixels);
        assert!(app.ensure_comic_composite_texture(&ctx, 0).is_none());
        let after_display = complete_comic(&mut app, &ctx);
        assert_eq!(after_display.pixels, before_display.pixels);
        let work = app.prepare_capture_pixel_work_for_test(&ctx, 0).unwrap();
        let crate::capture::CapturePixelWork::Single(job) = &work else {
            panic!("cover capture must preserve its single navigation page");
        };
        assert_eq!(job.source.pixels, after_export.pixels);
        let (_, after_width, after_height, after_rgba) =
            crate::capture::run_pixel_work(work).unwrap();
        assert_eq!((after_width, after_height), (before_width, before_height));
        assert_eq!(after_rgba, before_rgba);
        assert!(!app.fs_cache.contains_key(&0));
        assert!(app.edit_result_cache.is_empty());
        assert!(app.fs_pending.is_empty());
    }
}

#[test]
fn composite_pixels_revision_exact_gate_rejects_incomplete_and_stale_entries() {
    let ctx = egui::Context::default();
    let (mut app, keys) = warm_final_pair(&ctx);
    evict_pair_cpu(&mut app);
    let key = keys[1];
    assert!(app.exact_complete_final_composite_entry(0).is_some());
    app.final_composite_cache.get_mut(&key).unwrap().complete = false;
    assert!(app.exact_complete_final_composite_entry(0).is_none());
    app.final_composite_cache.get_mut(&key).unwrap().complete = true;
    app.input_generation.insert(0, key.edit_key.source_gen + 1);
    assert!(app.exact_complete_final_composite_entry(0).is_none());
    app.input_generation.insert(0, key.edit_key.source_gen);
    app.erase_mask_generation
        .insert(0, key.edit_key.erase_mask_gen + 1);
    assert!(app.exact_complete_final_composite_entry(0).is_none());
    app.erase_mask_generation
        .insert(0, key.edit_key.erase_mask_gen);
    app.local_adjust_generation
        .insert(0, key.edit_key.local_gen + 1);
    assert!(app.exact_complete_final_composite_entry(0).is_none());
    app.local_adjust_generation
        .insert(0, key.edit_key.local_gen);
    app.conceal_mask_generation
        .insert(0, key.edit_key.conceal_mask_gen + 1);
    assert!(app.exact_complete_final_composite_entry(0).is_none());
    app.conceal_mask_generation
        .insert(0, key.edit_key.conceal_mask_gen);
    app.conceal_generation += 1;
    assert!(app.exact_complete_final_composite_entry(0).is_none());
    app.conceal_generation -= 1;
    app.settings.global_preset.brightness += 1.0;
    assert!(app.exact_complete_final_composite_entry(0).is_none());
    app.settings.global_preset.brightness -= 1.0;
    app.post_filter_bypassed = !app.post_filter_bypassed;
    assert!(app.exact_complete_final_composite_entry(0).is_none());
    app.post_filter_bypassed = !app.post_filter_bypassed;
    let ai_mode = app.settings.ai_feature_mode;
    app.settings.ai_feature_mode = crate::settings::AiFeatureMode::Disabled;
    assert!(app.exact_complete_final_composite_entry(0).is_none());
    app.settings.ai_feature_mode = ai_mode;
    app.fs_transparent_bg_mode = 1;
    assert!(app.exact_complete_final_composite_entry(0).is_none());
    app.fs_transparent_bg_mode = 0;
    app.items_generation += 1;
    assert!(app.exact_complete_final_composite_entry(0).is_none());
    app.items_generation -= 1;
    let item = app.items[0].clone();
    app.items[0] = GridItem::Image(app.tmp.path().join("different-page.png"));
    assert!(app.exact_complete_final_composite_entry(0).is_none());
    app.items[0] = item;
    assert!(app.exact_complete_final_composite_entry(0).is_some());

    let texture = app
        .final_composite_cache
        .get(&keys[0])
        .unwrap()
        .texture
        .clone();
    assert!(app.display_texture_matches_page(&texture, 21));
    app.final_composite_cache.get_mut(&key).unwrap().texture = texture;
    let _expected = crate::item_identity::expect_mismatch();
    assert!(app.exact_complete_final_composite_entry(0).is_none());
    assert!(app.display_identity_error.borrow().is_some());
}

#[test]
fn composite_pixels_revision_animated_source_still_rejects_live_final_pixels() {
    let ctx = egui::Context::default();
    let (mut app, keys) = warm_final_pair(&ctx);
    let entry = app.final_composite_cache.get(&keys[1]).unwrap();
    let tex = entry.texture.clone();
    let pixels = Arc::clone(&entry.pixels);
    app.fs_cache.insert(
        0,
        FsCacheEntry::Static {
            tex,
            pixels,
            source_dims: Some([4, 6]),
            load_seq: 1,
            animation: StaticAnimationState::FirstFrameOnly(
                crate::canonical_image_loader::CanonicalAnimatedFormat::Gif,
            ),
        },
    );
    assert!(app.exact_complete_final_composite_entry(0).is_none());
    assert!(
        app.ensure_final_composite_pixels_with_key(&ctx, 0)
            .is_none()
    );
}

#[cfg(windows)]
#[test]
fn composite_pixels_revision_readers_consume_only_their_own_live_pixels() {
    let ctx = egui::Context::default();
    let (mut app, keys) = warm_final_pair(&ctx);
    evict_pair_cpu(&mut app);
    let main_pixels = Arc::clone(&app.final_composite_cache.get(&keys[1]).unwrap().pixels);
    let main_texture = app
        .final_composite_cache
        .get(&keys[1])
        .unwrap()
        .texture
        .id();
    let items = app.items.clone();
    let folder = app.current_folder.clone();
    let sibling = app.build_window_context_for_test(29_342, |owner| {
        owner.items = items;
        owner.current_folder = folder;
        owner
            .top_level_grid_view
            .replace_surface(top_level_grid_view::TopLevelGridSurface::Folder);
        owner.visible_indices = (0..22).collect();
        owner.details_order = (0..22).collect();
        owner.fullscreen_idx = Some(19);
        owner.spread_mode = crate::settings::SpreadMode::LtrCover;
        owner.final_cover_spread_preference = crate::settings::FinalCoverSpreadPreference::On;
        owner.viewer_navigation_caches.invalidate();
        assert!(owner.exact_complete_final_composite_entry(0).is_none());
        let key = retain_pdf_ai(owner, 0);
        let params = owner.effective_params(0);
        let ai_key = owner
            .final_ai_key_for_pixels(key.edit_key, [4, 6], params)
            .unwrap();
        let retained_key = owner.retained_final_ai_key_for(0, ai_key, [4, 6]).unwrap();
        assert!(owner.insert_retained_pdf_final_ai(
            0,
            retained_key,
            Arc::new(egui::ColorImage::filled([8, 12], egui::Color32::BLUE)),
        ));
        owner.prefetch_retained_paged_composites(&ctx, 19);
        super::paged_prefetch_tests::complete_pending_composites_for_display(owner, &ctx);
        evict_pair_cpu(owner);
    });
    app.with_window_context_for_test(sibling, |owner| {
        let (_, pixels) = owner
            .ensure_final_composite_pixels_with_key(&ctx, 0)
            .unwrap();
        assert!(
            pixels
                .pixels
                .iter()
                .all(|pixel| *pixel == egui::Color32::BLUE)
        );
        assert!(!Arc::ptr_eq(&pixels, &main_pixels));
        assert!(!owner.fs_cache.contains_key(&0));
        assert!(owner.edit_result_cache.is_empty());
        owner.input_generation.insert(0, 77);
        assert!(owner.exact_complete_final_composite_entry(0).is_none());
    });
    let (_, pixels) = app.ensure_final_composite_pixels_with_key(&ctx, 0).unwrap();
    assert!(Arc::ptr_eq(&pixels, &main_pixels));
    assert_eq!(
        app.final_composite_cache
            .get(&keys[1])
            .unwrap()
            .texture
            .id(),
        main_texture
    );
    assert!(app.fs_pending.is_empty());
    assert!(app.final_effect_pending.is_empty());
}
