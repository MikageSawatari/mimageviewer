use super::*;

#[test]
fn canonical_native_warm_collection_and_revalidation_do_not_scan_book_or_build_slideshow() {
    let ctx = egui::Context::default();
    let mut app = super::tests::ready_app(&ctx, SpreadMode::LtrCover);
    app.current_folder = Some(std::path::PathBuf::from("C:/d3-native-topology"));
    app.spread_display_composition_for_anchor(0);
    assert_eq!(
        app.spread_display_units_cache.complete_page_permutation,
        Some(true)
    );
    for slideshow in [false, true] {
        app.slideshow_playing = slideshow;
        reset_native_topology_work_counts_for_test();
        for _ in 0..100 {
            super::super::page_turn_submission_tests::frame(&ctx, |ui, ctx| {
                app.record_next_canonical_native_demand(ui);
                let demand = app
                    .collect_next_canonical_demand(ctx, super::tests::view(ui))
                    .unwrap();
                assert!(app.canonical_unit_demand_is_current(ctx, &demand));
            });
        }
        assert_eq!(native_topology_work_counts_for_test(), [0; 5]);
    }
}

#[test]
fn canonical_native_missing_complete_proof_or_replaced_nav_owner_is_not_rebuilt() {
    let ctx = egui::Context::default();
    let mut app = super::tests::ready_app(&ctx, SpreadMode::RtlCover);
    app.current_folder = Some(std::path::PathBuf::from("C:/d3-native-topology"));
    app.spread_display_composition_for_anchor(0);
    super::super::page_turn_submission_tests::frame(&ctx, |ui, ctx| {
        app.record_next_canonical_native_demand(ui);
        let demand = app
            .collect_next_canonical_demand(ctx, super::tests::view(ui))
            .unwrap();
        let proof = app
            .spread_display_units_cache
            .complete_page_permutation
            .take();
        assert_eq!(proof, Some(true));
        reset_native_topology_work_counts_for_test();
        assert_eq!(
            app.collect_next_canonical_demand(ctx, super::tests::view(ui)),
            Err(CanonicalDemandUnavailable::Topology)
        );
        assert!(!app.canonical_unit_demand_is_current(ctx, &demand));
        app.spread_display_units_cache.complete_page_permutation = proof;
        assert!(app.canonical_unit_demand_is_current(ctx, &demand));
        let nav = app.viewer_navigation_caches.nav_indices.as_ref().unwrap();
        app.viewer_navigation_caches.nav_indices = Some(Arc::new(nav.as_ref().clone()));
        assert_eq!(
            app.collect_next_canonical_demand(ctx, super::tests::view(ui)),
            Err(CanonicalDemandUnavailable::Topology)
        );
        assert!(!app.canonical_unit_demand_is_current(ctx, &demand));
        assert_eq!(native_topology_work_counts_for_test(), [0; 5]);
    });
}

#[test]
fn canonical_native_slideshow_skips_unadopted_still_topology_without_rebuilding() {
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![
        GridItem::Image(std::path::PathBuf::from("C:/d3/a.png")),
        GridItem::Video(std::path::PathBuf::from("C:/d3/video.mp4")),
        GridItem::Image(std::path::PathBuf::from("C:/d3/b.png")),
        GridItem::Image(std::path::PathBuf::from("C:/d3/c.png")),
    ];
    app.visible_indices = vec![0, 1, 2, 3];
    app.details_order = app.visible_indices.clone();
    app.spread_mode = SpreadMode::Single;
    let nav = app.get_nav_indices();
    app.build_spread_display_units_for_nav(&nav);
    app.slideshow_playing = true;
    reset_native_topology_work_counts_for_test();
    assert!(
        app.resident_final_unit_composition_from_snapshot(0, FinalUnitPosition::Next)
            .is_none()
    );
    assert_eq!(native_topology_work_counts_for_test(), [0; 5]);
    // Ordinary slideshow navigation can adopt its still-only topology. Reading
    // that owner is then allowed without changing the visible topology here.
    let still = app.get_still_image_indices();
    app.build_spread_display_units_for_nav(&still);
    reset_native_topology_work_counts_for_test();
    let demand = app
        .resident_final_unit_composition_from_snapshot(0, FinalUnitPosition::Next)
        .unwrap()
        .unwrap();
    assert_eq!(demand.navigation_anchor_idx, 2);
    assert_eq!(native_topology_work_counts_for_test(), [0; 5]);
    app.slideshow_playing = false;
    assert!(
        app.resident_final_unit_composition_from_snapshot(0, FinalUnitPosition::Next)
            .is_none(),
        "a slideshow-only topology cannot authorize manual navigation after stop"
    );
    assert_eq!(native_topology_work_counts_for_test(), [0; 5]);
}
