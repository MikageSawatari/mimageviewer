//! D3 CPU demand uses the real producer/keep/adoption boundaries, not only the
//! Native resident collector. Setup and legitimate cold work are outside spies.
use super::*;
use crate::app::final_prepare::tests::{begin_registration_frame, wait_ready};
use crate::ui_fullscreen::page_turn_submission_tests::{
    draw_current_without_sources, frame, install_presentation,
};

impl App {
    pub(crate) fn forget_final_demand_topologies_for_test(&mut self) {
        self.spread_display_units_cache.final_demand_topologies = None;
    }

    pub(crate) fn has_final_demand_topologies_for_test(&self) -> bool {
        self.spread_display_units_cache
            .final_demand_topologies
            .is_some()
    }
}

fn seed(app: &mut App, ctx: &egui::Context, mode: SpreadMode, mixed: bool) {
    app.items_generation += 1;
    app.items = (0..4)
        .map(|idx| GridItem::Image(PathBuf::from(format!("C:/d3-topology/{idx}.png"))))
        .collect();
    if mixed {
        app.items[1] = GridItem::Video(PathBuf::from("C:/d3-topology/media.mp4"));
    }
    app.current_folder = Some(PathBuf::from("C:/d3-topology"));
    app.visible_indices = vec![0, 1, 2, 3];
    app.details_order = app.visible_indices.clone();
    app.thumbnails = vec![crate::grid_item::ThumbnailState::Pending; 4];
    app.image_metas = vec![None; 4];
    app.fullscreen_idx = Some(0);
    app.spread_mode = mode;
    app.reading_flow = ReadingFlow::Paged;
    app.slideshow_playing = false;
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.settings.still_seek_strip_visible = false;
    app.settings.margin_fit_enabled = false;
    app.fs_cache.clear();
    app.final_composite_cache.clear();
    app.adjustment_page_params.clear();
    app.rotation_cache.clear();
    app.spread_display_units_cache = Default::default();
    app.viewer_navigation_caches.invalidate();
    for idx in 0..4 {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
        let pixels = Arc::new(egui::ColorImage::filled(
            [80, 120],
            egui::Color32::LIGHT_BLUE,
        ));
        let tex = ctx.load_texture(
            format!("d3-topology-{}-{idx}", app.items_generation),
            pixels.clone(),
            crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex,
                pixels,
                source_dims: Some([80, 120]),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.record_page_dims_for_spread(idx, (80, 120));
        if idx != 0 {
            let mut params = app.effective_params(idx).clone();
            params.brightness = 15.0;
            app.adjustment_page_params.insert(idx, params);
        }
    }
    let _ = app.page_turn_prefetch_targets(0, 2, 2);
}

fn ready(ctx: &egui::Context, mode: SpreadMode, mixed: bool) -> crate::app::AppTestEnvForTest {
    install_presentation(ctx);
    let mut app = crate::app::setup_app_for_test();
    seed(&mut app, ctx, mode, mixed);
    frame(ctx, |ui, ctx| {
        draw_current_without_sources(&mut app, ui, ctx);
    });
    assert!(app.page_turn_current_unit_presented());
    app
}

fn reset_work() {
    reset_native_topology_work_counts_for_test();
    reset_nav_indices_build_count_for_test();
}

fn assert_no_work() {
    assert_eq!(native_topology_work_counts_for_test(), [0; 5]);
    assert_eq!(nav_indices_build_count_for_test(), 0);
}

fn assert_current(app: &App) {
    let saved = app
        .spread_display_units_cache
        .final_demand_topologies
        .as_ref()
        .unwrap();
    assert!(app.final_demand_topologies_are_current(saved));
}

fn demand_pages(app: &App) -> Vec<usize> {
    app.next_final_prepare_occurrences(0)
        .unwrap()
        .into_iter()
        .map(|page| page.idx)
        .collect()
}

#[test]
fn d3_5_cpu_producer_keep_and_ready_adoption_are_warm_without_book_work() {
    for mixed in [false, true] {
        for mode in [
            SpreadMode::Single,
            SpreadMode::Ltr,
            SpreadMode::Rtl,
            SpreadMode::LtrCover,
            SpreadMode::RtlCover,
        ] {
            let ctx = egui::Context::default();
            let mut app = ready(&ctx, mode, mixed);
            app.slideshow_playing = mixed;
            app.prepare_final_demand_topologies();
            let wanted = demand_pages(&app);
            assert!(!wanted.is_empty());
            // Run actual CPU producers to CpuReady, including both pages of a
            // Pair. BeforeLookup must retain those results for the final hook.
            for idx in &wanted {
                app.prefetch_next_final_unit(&ctx, 0);
                wait_ready(&mut app, *idx);
            }
            let source = app.page_turn_prefetch_targets(0, 2, 2).unwrap().targets;
            let ai = app.ai_prefetch_targets(0);
            let held = app.page_turn_gpu_fingerprint_for_test();
            reset_work();
            for _ in 0..100 {
                app.prefetch_next_final_unit(&ctx, 0);
                let keep = app.final_demand_keep_for_test(0).unwrap();
                for idx in &wanted {
                    assert!(keep.contains(idx));
                }
                app.poll_final_effects(&ctx);
                assert_eq!(demand_pages(&app), wanted);
                assert_eq!(app.final_effect_pending.len(), wanted.len());
            }
            assert_no_work();
            assert_eq!(app.page_turn_gpu_fingerprint_for_test(), held);
            assert_eq!(
                app.page_turn_prefetch_targets(0, 2, 2).unwrap().targets,
                source
            );
            assert_eq!(app.ai_prefetch_targets(0), ai);
        }
    }
}

#[test]
fn d3_5_mixed_still_topology_and_normal_media_boundary_preserve_visible_proof() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, mode, true);
        let units = app.spread_display_units_cache.units.clone();
        let token = app.spread_display_units_cache.token.clone();
        let proof = app.spread_display_units_cache.complete_page_permutation;
        let held = app.page_turn_gpu_fingerprint_for_test();
        app.rotation_cache.clear();
        app.prepare_final_demand_topologies();
        assert!(
            app.rotation_cache.is_empty(),
            "pure cold demand must not hydrate rotation"
        );
        assert!(demand_pages(&app).is_empty());
        app.slideshow_playing = true;
        assert_eq!(
            demand_pages(&app),
            if mode == SpreadMode::Single {
                vec![2]
            } else {
                vec![3]
            }
        );
        app.slideshow_playing = false;
        assert!(
            demand_pages(&app).is_empty(),
            "manual next is the video unit"
        );
        assert!(Arc::ptr_eq(&units, &app.spread_display_units_cache.units));
        assert_eq!(token, app.spread_display_units_cache.token);
        assert_eq!(
            proof,
            app.spread_display_units_cache.complete_page_permutation
        );
        assert_eq!(held, app.page_turn_gpu_fingerprint_for_test());
        let saved = app
            .spread_display_units_cache
            .final_demand_topologies
            .as_ref()
            .unwrap();
        assert!(matches!(saved.normal, FinalDemandTopology::Visible(_)));
        assert!(matches!(saved.still, FinalDemandTopology::Derived(_)));
        reset_work();
        for playing in [true, false, true, false] {
            app.slideshow_playing = playing;
            app.prepare_final_demand_topologies();
            assert!(app.next_final_prepare_occurrences(0).is_some());
        }
        assert_no_work();
    }
}

#[test]
fn d3_5_visible_still_adoption_borrows_its_index_and_keeps_a_distinct_normal_topology() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, SpreadMode::Ltr, true);
    app.slideshow_playing = true;
    let still = app.get_still_image_indices();
    let visible = app.build_spread_display_units_for_nav(&still);
    let token = app.spread_display_units_cache.token.clone();
    app.prepare_final_demand_topologies();
    assert_current(&app);
    let saved = app
        .spread_display_units_cache
        .final_demand_topologies
        .as_ref()
        .unwrap();
    assert!(matches!(saved.normal, FinalDemandTopology::Derived(_)));
    assert!(matches!(saved.still, FinalDemandTopology::Visible(_)));
    assert_eq!(demand_pages(&app), vec![3]);
    app.slideshow_playing = false;
    assert!(demand_pages(&app).is_empty());
    reset_work();
    for _ in 0..100 {
        app.prepare_final_demand_topologies();
        assert!(app.current_final_prepare_occurrences(0).is_some());
    }
    assert_no_work();
    assert!(Arc::ptr_eq(&visible, &app.spread_display_units_cache.units));
    assert_eq!(token, app.spread_display_units_cache.token);
}

#[test]
fn d3_5_root_direct_adoption_rebuilds_cold_same_content_nav_and_rejects_obsolete_unit() {
    for changed_demand in [false, true] {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, SpreadMode::Single, false);
        app.prefetch_next_final_unit(&ctx, 0);
        let key = wait_ready(&mut app, 1);
        if changed_demand {
            app.visible_indices = vec![0, 2, 1, 3];
            app.details_order = app.visible_indices.clone();
            app.viewer_navigation_caches.invalidate();
            let nav = app.get_nav_indices();
            app.build_spread_display_units_for_nav(&nav);
        } else {
            let nav = app.viewer_navigation_caches.nav_indices.as_ref().unwrap();
            app.viewer_navigation_caches.nav_indices = Some(Arc::new(nav.as_ref().clone()));
        }
        let units = app.spread_display_units_cache.units.clone();
        let token = app.spread_display_units_cache.token.clone();
        reset_work();
        begin_registration_frame(&mut app, 5_501);
        app.observe_live_paged_for_final_prepare();
        // No poll/producer/keep prefix after the owner replacement. The real
        // ROOT direct collect -> adopt must prepare its own cold snapshot.
        eframe::App::on_native_wgpu_root_output_finalized(&mut *app, &ctx, 5_501, 1);
        assert!(!app.final_effect_pending.contains_key(&key));
        assert_eq!(
            app.exact_complete_final_composite_texture(1).is_some(),
            !changed_demand
        );
        assert!(nav_indices_build_count_for_test() > 0);
        assert_current(&app);
        assert!(Arc::ptr_eq(&units, &app.spread_display_units_cache.units));
        assert_eq!(token, app.spread_display_units_cache.token);
        reset_work();
        app.prepare_final_demand_topologies();
        assert!(app.next_final_prepare_occurrences(0).is_some());
        app.poll_final_effects(&ctx);
        assert_no_work();
    }
}

#[test]
fn d3_5_cold_binding_tracks_reader_source_rotation_mode_and_page_alone() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, SpreadMode::LtrCover, false);
    app.prepare_final_demand_topologies();
    assert_current(&app);
    let old_units = app.spread_display_units_cache.units.clone();
    // A current reader order that no longer agrees with the visible baseline
    // cannot be certified merely by matching items generation and spread mode.
    app.visible_indices.reverse();
    app.details_order = app.visible_indices.clone();
    app.viewer_navigation_caches.invalidate();
    app.prepare_final_demand_topologies();
    assert!(app.next_final_prepare_occurrences(0).is_none());
    assert!(Arc::ptr_eq(
        &old_units,
        &app.spread_display_units_cache.units
    ));
    let nav = app.get_nav_indices();
    app.build_spread_display_units_for_nav(&nav);
    app.prepare_final_demand_topologies();
    assert_current(&app);
    let generation = app.items_generation;
    app.spread_display_units_cache
        .mark_rotation_for_recheck(generation, 0);
    assert!(app.next_final_prepare_occurrences(0).is_none());
    app.prepare_final_demand_topologies();
    assert!(app.next_final_prepare_occurrences(0).is_none());
    app.build_spread_display_units_for_nav(&nav);
    app.prepare_final_demand_topologies();
    assert_current(&app);
    app.record_page_dims_for_spread(0, (160, 80));
    assert!(app.next_final_prepare_occurrences(0).is_none());
    app.build_spread_display_units_for_nav(&nav);
    app.prepare_final_demand_topologies();
    assert_current(&app);
    app.settings.page_after_cover_alone_enabled = !app.settings.page_after_cover_alone_enabled;
    assert!(app.next_final_prepare_occurrences(0).is_none());
    app.build_spread_display_units_for_nav(&nav);
    app.prepare_final_demand_topologies();
    assert_current(&app);
    app.spread_mode = SpreadMode::Rtl;
    assert!(app.next_final_prepare_occurrences(0).is_none());
    app.build_spread_display_units_for_nav(&nav);
    app.prepare_final_demand_topologies();
    assert_current(&app);
    app.set_spread_shift_anchor_for_test(Some(2));
    assert!(app.next_final_prepare_occurrences(0).is_none());
    app.build_spread_display_units_for_nav(&nav);
    app.prepare_final_demand_topologies();
    assert_current(&app);
    app.items_generation += 1;
    assert!(app.next_final_prepare_occurrences(0).is_none());
    app.viewer_navigation_caches.invalidate();
    let nav = app.get_nav_indices();
    app.build_spread_display_units_for_nav(&nav);
    app.prepare_final_demand_topologies();
    assert_current(&app);
    reset_work();
    for _ in 0..100 {
        app.prepare_final_demand_topologies();
    }
    assert_no_work();
}

#[test]
fn d3_5_derived_composed_overrides_follow_the_accepted_token_without_replacing_visible_units() {
    use crate::book_composition::{
        CanonicalRawPage, CompositionProjectionOutcome, CompositionToken, PageDomain, PageOverride,
        PageOverrideEntry, PageSide, ReadingDirection as CompositionDirection, fixtures,
        project_composition,
    };
    let mut book = fixtures::four_page_book(CompositionDirection::Ltr);
    book.cover_enabled = true;
    book.page_overrides.push(PageOverrideEntry {
        page: fixtures::document_page(2),
        directive: PageOverride::BlankOpposite {
            page_side: PageSide::Leading,
        },
    });
    let mut app = super::book_composition_tests::composed_app(&book);
    app.settings.page_turn_animation_enabled = true;
    app.fullscreen_idx = Some(0);
    app.settings.page_after_cover_alone_enabled = false;
    app.settings.last_page_alone_enabled = false;
    for idx in 0..4 {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
    }
    let nav = app.get_nav_indices();
    app.build_spread_display_units_for_nav(&nav);
    let visible = app.spread_display_units_cache.units.clone();
    app.settings.page_after_cover_alone_enabled = true;
    app.prepare_final_demand_topologies();
    let saved = app
        .spread_display_units_cache
        .final_demand_topologies
        .as_ref()
        .unwrap();
    let FinalDemandTopology::Derived(derived) = &saved.normal else {
        panic!("the changed policy needs an independent derived shape");
    };
    assert!(matches!(
        derived.units[*derived.positions.get(&2).unwrap()].formation,
        SpreadDisplayUnitFormation::Singleton(SpreadDisplaySingletonCause::ExplicitBlankOpposite(
            _
        ))
    ));
    assert!(Arc::ptr_eq(&visible, &app.spread_display_units_cache.units));
    book.page_overrides[0].directive = PageOverride::Alone;
    let raw = (0..4)
        .map(|page| CanonicalRawPage {
            page: fixtures::document_page(page),
            raw_index: page as usize,
        })
        .collect::<Vec<_>>();
    let prepared = project_composition(
        crate::book_scope::BookScopeKey::document("c:/g3-display.pdf").unwrap(),
        &PageDomain::Document,
        CompositionToken {
            service_epoch: 3,
            publication_sequence: 8,
        },
        Some(&book),
        &raw,
        &fixtures::document_manifest(4),
        4,
    )
    .unwrap();
    let CompositionProjectionOutcome::Composed(projection) = prepared.outcome else {
        panic!()
    };
    app.install_composition_for_test(projection);
    assert!(app.next_final_prepare_occurrences(0).is_none());
    app.prepare_final_demand_topologies();
    assert!(
        app.next_final_prepare_occurrences(0).is_none(),
        "old visible composed token cannot certify the new override"
    );
    app.build_spread_display_units_for_nav(&nav);
    let visible = app.spread_display_units_cache.units.clone();
    app.settings.page_after_cover_alone_enabled = false;
    app.prepare_final_demand_topologies();
    let saved = app
        .spread_display_units_cache
        .final_demand_topologies
        .as_ref()
        .unwrap();
    let FinalDemandTopology::Derived(derived) = &saved.normal else {
        panic!()
    };
    assert!(matches!(
        derived.units[*derived.positions.get(&2).unwrap()].formation,
        SpreadDisplayUnitFormation::Singleton(SpreadDisplaySingletonCause::ExplicitAlone)
    ));
    assert!(Arc::ptr_eq(&visible, &app.spread_display_units_cache.units));
    reset_work();
    for _ in 0..100 {
        app.prepare_final_demand_topologies();
        assert!(app.next_final_prepare_occurrences(0).is_some());
    }
    assert_no_work();
}

#[test]
fn d3_5_ineligible_and_empty_receipt_do_not_eagerly_build_or_block_legacy_adoption() {
    for route in ["off", "list", "vertical", "split"] {
        let ctx = egui::Context::default();
        let mut app = ready(&ctx, SpreadMode::Single, false);
        let key = crate::app::final_prepare_tests::queued_final(&mut app, 0);
        match route {
            "off" => app.settings.page_turn_animation_enabled = false,
            "list" => app.fullscreen_idx = None,
            "vertical" => app.reading_flow = ReadingFlow::Vertical,
            "split" => app.spread_mode = SpreadMode::SplitLtr,
            _ => unreachable!(),
        }
        app.spread_display_units_cache.final_demand_topologies = None;
        reset_work();
        app.poll_final_effects(&ctx);
        assert!(!app.final_effect_pending.contains_key(&key));
        assert!(app.final_composite_cache.contains_key(&key));
        assert!(
            app.spread_display_units_cache
                .final_demand_topologies
                .is_none()
        );
        assert_no_work();
    }
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, SpreadMode::Single, false);
    app.spread_display_units_cache.final_demand_topologies = None;
    reset_work();
    app.poll_final_effects(&ctx);
    assert!(
        app.spread_display_units_cache
            .final_demand_topologies
            .is_none()
    );
    assert_no_work();
}

#[cfg(windows)]
#[test]
fn d3_5_derived_topology_is_local_across_mount_fork_and_sibling_mutations() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, SpreadMode::Single, false);
    app.prepare_final_demand_topologies();
    let main_units = app.spread_display_units_cache.units.clone();
    let main_nav = app
        .spread_display_units_cache
        .final_demand_topologies
        .as_ref()
        .unwrap()
        .normal_nav
        .clone();
    let held = app.page_turn_gpu_fingerprint_for_test();
    let sibling = app.build_window_context_for_test(95_501, |owner| {
        assert!(
            owner
                .spread_display_units_cache
                .final_demand_topologies
                .is_none(),
            "fork starts with fresh cache"
        );
        seed(owner, &ctx, SpreadMode::Ltr, true);
        owner.slideshow_playing = true;
        owner.prepare_final_demand_topologies();
        assert_eq!(demand_pages(owner), vec![3]);
    });
    let sibling_units = app.with_window_context_for_test(sibling, |owner| {
        owner.spread_display_units_cache.units.clone()
    });
    for _ in 0..3 {
        app.with_window_context_for_test(sibling, |owner| {
            assert_current(owner);
            assert!(Arc::ptr_eq(
                &sibling_units,
                &owner.spread_display_units_cache.units
            ));
        });
        assert_current(&app);
        assert!(Arc::ptr_eq(
            &main_units,
            &app.spread_display_units_cache.units
        ));
        assert!(Arc::ptr_eq(
            &main_nav,
            &app.spread_display_units_cache
                .final_demand_topologies
                .as_ref()
                .unwrap()
                .normal_nav
        ));
        assert_eq!(app.page_turn_gpu_fingerprint_for_test(), held);
    }
    app.items_generation += 1;
    app.viewer_navigation_caches.invalidate();
    app.with_window_context_for_test(sibling, |owner| {
        reset_work();
        owner.prepare_final_demand_topologies();
        assert_eq!(demand_pages(owner), vec![3]);
        assert_current(owner);
        assert_no_work();
    });
}

#[cfg(windows)]
#[test]
fn d3_5_remote_skips_cold_derivation_and_park_mount_has_no_service_side_effect() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, SpreadMode::Single, false);
    let handle = crate::remote_ipc::session::SessionHandle::new();
    app.set_remote_session_handle(handle.clone());
    let _ = handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
        client_id: "d3-5-topology".into(),
        peer: mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: Some("test".into()),
        },
    });
    assert!(app.remote_session_blocks_local_control());
    let key = crate::app::final_prepare_tests::queued_final(&mut app, 0);
    app.spread_display_units_cache.final_demand_topologies = None;
    reset_work();
    app.poll_final_effects(&ctx);
    assert!(app.final_composite_cache.contains_key(&key));
    assert!(
        app.spread_display_units_cache
            .final_demand_topologies
            .is_none()
    );
    assert_no_work();
    assert!(handle.finish_acquire(handle.snapshot().generation));
    handle.local_disconnect();
    assert!(handle.complete_app_drain(handle.snapshot().generation));
    let sibling = app.build_window_context_for_test(95_502, |owner| {
        seed(owner, &ctx, SpreadMode::Single, false);
    });
    app.transition_detached_window_state(
        95_502,
        crate::app::DetachedWindowState::Parked,
        "d3-5-test",
    );
    app.with_window_context_for_test(sibling, |owner| {
        let key = crate::app::final_prepare_tests::queued_final(owner, 0);
        reset_work();
        // Mounting this parked owner is not an adoption service. A validated
        // resume is allowed to derive cold metadata in its later service.
        assert!(owner.final_effect_pending.contains_key(&key));
        assert!(!owner.final_composite_cache.contains_key(&key));
        assert!(
            owner
                .spread_display_units_cache
                .final_demand_topologies
                .is_none()
        );
        assert_no_work();
    });
    app.transition_detached_window_state(
        95_502,
        crate::app::DetachedWindowState::Resuming,
        "d3-5-test",
    );
    app.with_window_context_for_test(sibling, |owner| {
        owner.prepare_final_demand_topologies();
        assert_current(owner);
        reset_work();
        owner.prepare_final_demand_topologies();
        assert_no_work();
    });
}

#[cfg(windows)]
#[test]
fn d3_5_promote_moves_derived_proof_and_pending_without_changing_the_sibling() {
    let ctx = egui::Context::default();
    let mut app = ready(&ctx, SpreadMode::Single, false);
    app.prefetch_next_final_unit(&ctx, 0);
    let key = wait_ready(&mut app, 1);
    let units = app.spread_display_units_cache.units.clone();
    let nav = app
        .spread_display_units_cache
        .final_demand_topologies
        .as_ref()
        .unwrap()
        .normal_nav
        .clone();
    let held = app.page_turn_gpu_fingerprint_for_test();
    let original = app.final_demand_stash_for_test();
    assert!(
        app.spread_display_units_cache
            .final_demand_topologies
            .is_none()
    );
    assert!(app.final_effect_pending.is_empty());
    seed(&mut app, &ctx, SpreadMode::Ltr, true);
    app.slideshow_playing = true;
    app.prepare_final_demand_topologies();
    let sibling_units = app.spread_display_units_cache.units.clone();
    app.with_window_context_for_test(original, |owner| {
        reset_work();
        owner.prepare_final_demand_topologies();
        assert_current(owner);
        assert_no_work();
        assert!(Arc::ptr_eq(&units, &owner.spread_display_units_cache.units));
        assert!(Arc::ptr_eq(
            &nav,
            &owner
                .spread_display_units_cache
                .final_demand_topologies
                .as_ref()
                .unwrap()
                .normal_nav
        ));
        assert_eq!(owner.page_turn_gpu_fingerprint_for_test(), held);
        assert!(owner.final_effect_pending.contains_key(&key));
        assert!(owner.ensure_final_composite_texture(&ctx, 1).is_some());
        assert!(!owner.final_effect_pending.contains_key(&key));
    });
    assert_current(&app);
    assert!(Arc::ptr_eq(
        &sibling_units,
        &app.spread_display_units_cache.units
    ));
    assert_eq!(demand_pages(&app), vec![3]);
}
