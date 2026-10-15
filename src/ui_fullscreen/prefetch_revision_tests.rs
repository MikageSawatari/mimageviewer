#![cfg(test)]
use super::*;

fn mixed_book(mode: SpreadMode, media: usize) -> crate::app::AppTestEnvForTest {
    let mut app = crate::app::setup_app_for_test();
    let folder = app.tmp.path().join("mixed-prefetch");
    app.current_folder = Some(folder.clone());
    app.top_level_grid_view
        .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::Folder);
    app.settings.page_turn_animation_enabled = true;
    app.spread_mode = mode;
    let image_indices = [0, 1, media + 2, media + 3, media * 2 + 4, media * 2 + 5];
    app.items = (0..media * 2 + 6)
        .map(|idx| {
            if image_indices.contains(&idx) {
                GridItem::Image(folder.join(format!("{idx}.png")))
            } else {
                GridItem::Video(folder.join(format!("{idx}.mp4")))
            }
        })
        .collect();
    app.visible_indices = (0..app.items.len()).collect();
    app.details_order = app.visible_indices.clone();
    app.items_generation += 1;
    app.viewer_navigation_caches.invalidate();
    for idx in 0..app.items.len() {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
    }
    for idx in image_indices {
        app.record_page_dims_for_spread(idx, (4, 6));
    }
    app
}

#[test]
fn prefetch_revision_warm_demand_does_not_walk_large_media_intervals() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        let mut measurements = Vec::new();
        for media in [8, 8192] {
            let mut app = mixed_book(mode, media);
            if !mode.is_spread() {
                app.rotation_cache.clear();
            }
            let current = media + 2;
            let expected = if mode.is_spread() {
                vec![
                    (media + 1, media + 3),
                    (media * 2 + 2, media * 2 + 4),
                    (media * 2 + 2, media * 2 + 5),
                    (0, 0),
                    (0, 1),
                ]
            } else {
                vec![
                    (media + 3, media + 3),
                    (1, 1),
                    (media * 2 + 4, media * 2 + 4),
                    (0, 0),
                    (media * 2 + 5, media * 2 + 5),
                ]
            };
            assert_eq!(
                app.page_turn_prefetch_targets(current, 3, 2)
                    .unwrap()
                    .targets,
                expected
            );
            // Also cover a fresh navigation allocation with identical contents.
            app.viewer_navigation_caches.invalidate();
            assert_eq!(
                app.page_turn_prefetch_targets(current, 3, 2)
                    .unwrap()
                    .targets,
                expected
            );
            PREFETCH_NAV_COMPARISON_ELEMENTS.set(0);
            PREFETCH_POSITION_SCANS.set(0);
            PREFETCH_UNIT_VISITS.set(0);
            reset_spread_display_units_build_count_for_test();
            reset_final_cover_eligibility_scan_count_for_test();
            for _ in 0..4 {
                assert_eq!(
                    app.page_turn_prefetch_targets(current, 3, 2)
                        .unwrap()
                        .targets,
                    expected
                );
            }
            assert_eq!(
                PREFETCH_NAV_COMPARISON_ELEMENTS.get(),
                0,
                "no full nav comparison after preparation"
            );
            assert_eq!(PREFETCH_POSITION_SCANS.get(), 0, "no linear anchor search");
            assert_eq!(spread_display_units_build_count_for_test(), 0);
            assert_eq!(final_cover_eligibility_scan_count_for_test(), 0);
            measurements.push(PREFETCH_UNIT_VISITS.get());
            if !mode.is_spread() {
                assert!(
                    app.rotation_cache.is_empty(),
                    "single-page demand needs no rotation DB read"
                );
            }
        }
        assert_eq!(
            measurements[0], measurements[1],
            "media width must not change resolver visits"
        );
        assert_eq!(measurements[0], if mode.is_spread() { 12 } else { 24 });
    }
}

#[test]
fn prefetch_revision_arbitrary_nav_slices_keep_exact_validation_and_first_match() {
    let mut app = mixed_book(SpreadMode::Ltr, 8);
    let nav = app.get_nav_indices();
    let original = app.build_spread_display_units_for_nav(&nav);
    let mut independent = nav.as_ref().clone();
    assert!(Arc::ptr_eq(
        &original,
        &app.build_spread_display_units_for_nav(&independent)
    ));
    independent.swap(0, 2);
    let changed = app.build_spread_display_units_for_nav(&independent);
    assert!(!Arc::ptr_eq(&original, &changed));
    assert_eq!(app.spread_display_units_cache.unit_position(0), Some(1));
    let duplicate = vec![0, 2, 0];
    app.build_spread_display_units_for_nav(&duplicate);
    assert_eq!(app.spread_display_units_cache.unit_position(0), Some(0));
}

#[test]
fn prefetch_revision_single_paint_and_metadata_harvest_keep_one_topology() {
    let ctx = egui::Context::default();
    let mut app = mixed_book(SpreadMode::Single, 8);
    let current = 10;
    let pixels = Arc::new(egui::ColorImage::filled([8, 4], egui::Color32::WHITE));
    let texture = ctx.load_texture(
        "single-landscape-metadata",
        Arc::clone(&pixels),
        egui::TextureOptions::LINEAR,
    );
    app.fs_cache.insert(
        current,
        FsCacheEntry::Static {
            tex: texture,
            pixels,
            source_dims: Some([8, 4]),
            load_seq: 1,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    // Ordinary composition must prepare the allocation before a PaintedUnit pins it.
    let composition = app.spread_display_composition_for_anchor(current);
    assert_eq!(composition.navigation_pages(), vec![current]);
    let layout = Arc::clone(&app.spread_display_units_cache.units);
    assert!(!layout.is_empty());
    let epoch = app.spread_display_units_cache.landscape_epoch;
    let generation = app.items_generation;
    for _ in 0..4 {
        app.harvest_page_dims();
        app.spread_display_units_cache
            .mark_rotation_for_recheck(generation, current);
        app.spread_display_units_cache
            .mark_all_rotations_for_recheck(generation);
        app.page_turn_prefetch_targets(current, 3, 2).unwrap();
        app.spread_display_composition_for_anchor(current);
        assert!(Arc::ptr_eq(&layout, &app.spread_display_units_cache.units));
        assert_eq!(app.spread_display_units_cache.landscape_epoch, epoch);
        assert!(
            app.spread_display_units_cache
                .rotation_recheck_indices
                .is_empty()
        );
        assert!(!app.spread_display_units_cache.rotation_recheck_all);
    }
    app.spread_mode = SpreadMode::Ltr;
    app.spread_display_composition_for_anchor(current);
    assert!(!Arc::ptr_eq(&layout, &app.spread_display_units_cache.units));
    assert!(
        app.spread_display_units_cache
            .landscape_indices
            .contains(&current)
    );
}
