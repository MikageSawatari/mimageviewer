#![cfg(test)]

use super::*;

pub(crate) fn pdf_book(ctx: &egui::Context) -> AppTestEnvForTest {
    let mut app = setup_app_for_test();
    let pdf_path = app.tmp.path().join("prefetch-book.pdf");
    app.current_folder = Some(pdf_path.clone());
    app.top_level_grid_view
        .replace_surface(top_level_grid_view::TopLevelGridSurface::Folder);
    app.settings.page_turn_animation_enabled = true;
    app.settings.final_cover_spread_enabled = true;
    app.final_cover_spread_preference = crate::settings::FinalCoverSpreadPreference::On;
    app.spread_mode = crate::settings::SpreadMode::LtrCover;
    app.settings.prefetch_forward = 1;
    app.settings.prefetch_back = 0;
    app.settings.ai_upscale_prefetch_forward = 1;
    app.settings.ai_upscale_prefetch_back = 0;
    for idx in 0..22 {
        app.items.push(GridItem::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num: idx as u32,
            content_type: Some(crate::pdf_loader::PdfPageContentType::Vector),
        });
        let pixels = Arc::new(egui::ColorImage::filled([4, 6], egui::Color32::WHITE));
        let tex = ctx.load_texture(
            format!("paged-prefetch-{idx}"),
            Arc::clone(&pixels),
            DISPLAY_IMAGE_TEXTURE_OPTIONS,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex,
                pixels,
                source_dims: Some([4, 6]),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
    }
    app.visible_indices = (0..22).collect();
    app.details_order = (0..22).collect();
    app.viewer_navigation_caches.invalidate();
    app.fullscreen_idx = Some(19);
    app
}

pub(crate) fn retain_pdf_ai(app: &mut App, idx: usize) -> FinalCompositeKey {
    app.settings.ai_feature_mode = crate::settings::AiFeatureMode::HighQuality;
    app.settings.global_preset.upscale_model = Some(
        crate::ai::ModelKind::UpscaleRealEsrGeneralV3
            .as_str()
            .into(),
    );
    let edit_key = app.current_edit_result_key(idx);
    let params = app.effective_params(idx).clone();
    let ai_key = app
        .final_ai_key_for_pixels(edit_key, [4, 6], &params)
        .unwrap();
    let retained_key = app.retained_final_ai_key_for(idx, ai_key, [4, 6]).unwrap();
    let pixels = Arc::new(egui::ColorImage::filled([8, 12], egui::Color32::WHITE));
    assert!(app.insert_retained_pdf_final_ai(idx, retained_key, pixels));
    app.fs_cache.remove(&idx);
    app.final_composite_key_for_pixels(edit_key, [4, 6], &params)
}

pub(crate) fn complete_pending_composites(app: &mut App, ctx: &egui::Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !app.final_effect_pending.is_empty() {
        app.poll_final_effects(ctx);
        assert!(
            std::time::Instant::now() < deadline,
            "composite worker did not finish"
        );
        std::thread::yield_now();
    }
}

#[test]
fn paged_prefetch_consumers_prepare_and_keep_the_upcoming_final_cover() {
    let ctx = egui::Context::default();
    let mut app = pdf_book(&ctx);
    assert_eq!(app.ai_prefetch_targets(19), vec![20, 21, 0]);
    let keep = app.compute_keep_set(19);
    assert!(keep.contains(&0) && keep.contains(&21));
    app.update_prefetch_window(19);
    assert!(app.fs_cache.contains_key(&0) && app.fs_cache.contains_key(&21));
    assert_eq!(app.displayed_spread_partner(19), Some(20));
}

#[test]
fn paged_prefetch_indicator_places_a_reused_cover_ahead_of_the_reader() {
    let ctx = egui::Context::default();
    let mut app = pdf_book(&ctx);
    app.settings.global_preset.upscale_model = Some(
        crate::ai::ModelKind::UpscaleRealEsrGeneralV3
            .as_str()
            .into(),
    );
    app.settings.ai_feature_mode = crate::settings::AiFeatureMode::HighQuality;
    let indicator = app.final_ai_prefetch_indicator(19).unwrap();
    assert!(indicator.behind.is_empty());
    assert_eq!(indicator.ahead, vec![FsPrefetchPageState::Missing; 2]);
}

#[test]
fn paged_prefetch_off_preserves_positional_targets_and_keep() {
    let ctx = egui::Context::default();
    let mut app = pdf_book(&ctx);
    app.settings.page_turn_animation_enabled = false;
    assert_eq!(app.ai_prefetch_targets(19), vec![20]);
    assert!(!app.compute_keep_set(19).contains(&0));
    app.update_prefetch_window(19);
    assert!(!app.fs_cache.contains_key(&0));
}

#[test]
fn paged_prefetch_ai_window_keeps_its_actual_sources_when_source_budget_is_smaller() {
    let ctx = egui::Context::default();
    let mut app = pdf_book(&ctx);
    app.fullscreen_idx = Some(17);
    app.settings.prefetch_forward = 0;
    app.settings.prefetch_back = 0;
    app.settings.ai_upscale_prefetch_forward = 3;
    assert!(app.ai_prefetch_targets(17).contains(&0));
    assert!(app.compute_keep_set(17).contains(&0));
    app.update_prefetch_window(17);
    assert!(app.fs_cache.contains_key(&0));
}

#[test]
fn paged_prefetch_reconciles_a_changed_window_through_the_existing_source_owner() {
    let ctx = egui::Context::default();
    let mut app = pdf_book(&ctx);
    app.reconcile_paged_prefetch_window(19);
    assert!(app.fs_cache.contains_key(&0) && app.fs_cache.contains_key(&21));
    assert!(!app.fs_cache.contains_key(&10));
    let resident = app.fs_cache.keys().copied().collect::<HashSet<_>>();
    app.reconcile_paged_prefetch_window(19);
    assert_eq!(
        app.fs_cache.keys().copied().collect::<HashSet<_>>(),
        resident
    );
    assert!(app.fs_pending.is_empty());
}

#[test]
fn paged_prefetch_retained_cover_becomes_resident_with_zero_inference_budget() {
    let ctx = egui::Context::default();
    let mut app = pdf_book(&ctx);
    app.settings.ai_upscale_prefetch_forward = 0;
    app.settings.ai_upscale_prefetch_back = 0;
    let key = retain_pdf_ai(&mut app, 0);
    assert_eq!(
        app.fs_page_load_state(0),
        FsPageLoadState::DisplayReady(FsPageDisplaySource::RetainedPdfFinalAi)
    );
    assert!(app.is_idx_final_ai_done_or_skipped(0));
    let retained_bytes = app.retained_cross_session_cache_bytes();
    app.prefetch_retained_paged_composites(&ctx, 19);
    assert!(app.final_effect_pending.contains_key(&key));
    assert_eq!(app.final_effect_pending.len(), 1);
    assert!(!app.final_composite_cache.contains_key(&key));
    assert!(!app.fs_pending.contains_key(&0));
    assert!(app.final_ai_pending.is_empty());
    app.prefetch_retained_paged_composites(&ctx, 19);
    assert_eq!(app.final_effect_pending.len(), 1);
    complete_pending_composites(&mut app, &ctx);
    assert!(
        app.final_composite_cache
            .get(&key)
            .is_some_and(|entry| entry.complete)
    );
    assert_eq!(app.retained_cross_session_cache_bytes(), retained_bytes);
    app.evict_final_pipeline_cache(19);
    assert!(app.final_composite_cache.contains_key(&key));
    app.prefetch_retained_paged_composites(&ctx, 19);
    assert!(app.final_effect_pending.is_empty());
}

fn retain_pdf_output(app: &mut App, idx: usize, output_size: [usize; 2]) -> FinalCompositeKey {
    let key = retain_pdf_ai(app, idx);
    let params = app.effective_params(idx);
    let ai_key = app
        .final_ai_key_for_pixels(key.edit_key, [4, 6], params)
        .unwrap();
    let retained_key = app.retained_final_ai_key_for(idx, ai_key, [4, 6]).unwrap();
    assert!(app.insert_retained_pdf_final_ai(
        idx,
        retained_key,
        Arc::new(egui::ColorImage::filled(output_size, egui::Color32::WHITE)),
    ));
    key
}

fn static_texture_id(app: &App, idx: usize) -> egui::TextureId {
    match app.fs_cache.get(&idx).unwrap() {
        FsCacheEntry::Static { tex, .. } => tex.id(),
        _ => panic!("fixture needs a static source"),
    }
}

fn prepare_retained_cover_raster(app: &mut App, color: egui::Color32) {
    let content_type = crate::pdf_loader::PdfPageContentType::Raster { w: 4, h: 6 };
    let GridItem::PdfPage {
        content_type: stored,
        ..
    } = &mut app.items[0]
    else {
        panic!("fixture needs a PDF cover");
    };
    *stored = Some(content_type);
    let target = crate::pdf_loader::PdfDisplayTarget {
        width_px: 4,
        height_px: 5,
        fit_mode: crate::pdf_loader::PdfDisplayFitMode::Page,
    };
    // Vector pages have a larger render floor. A native 4x6 raster intentionally
    // caps the request to the tiny retained fixture, guaranteeing zero PDF I/O.
    assert_eq!(
        crate::pdf_loader::display_render_long_edge(target, 4.0, 6.0, false, content_type),
        6
    );
    app.fs_pdf_display_target = Some(target);
    assert!(app.insert_retained_pdf_page_raster(
        0,
        [4, 6],
        Arc::new(egui::ColorImage::filled([4, 6], color)),
    ));
}

#[test]
fn paged_prefetch_retained_budget_pressure_does_not_pin_cpu_covers_or_discard_live_gpu() {
    for (max_entries, output_size) in [(1, [8, 12]), (10, [512, 384])] {
        for warm_gpu in [false, true] {
            let ctx = egui::Context::default();
            let mut app = pdf_book(&ctx);
            app.settings.retained_final_ai_cache_max_entries = max_entries;
            app.settings.retained_final_ai_cache_max_mib = 1;
            let cover = retain_pdf_output(&mut app, 0, output_size);
            if warm_gpu {
                app.prefetch_retained_paged_composites(&ctx, 19);
                complete_pending_composites(&mut app, &ctx);
                assert!(app.final_composite_cache.contains_key(&cover));
            }
            retain_pdf_output(&mut app, 10, output_size);
            assert!(!app.has_retained_pdf_final_ai_for_current_params(0));
            assert!(app.retained_cross_session_cache_entries() <= max_entries);
            assert!(app.retained_cross_session_cache_bytes() <= 1024 * 1024);
            app.prefetch_retained_paged_composites(&ctx, 19);
            assert!(app.final_effect_pending.is_empty());
            app.evict_final_pipeline_cache(19);
            assert_eq!(app.final_composite_cache.contains_key(&cover), warm_gpu);
            // Use a retained raster as the existing source producer, never real PDF I/O.
            prepare_retained_cover_raster(&mut app, egui::Color32::WHITE);
            let before_input = app.current_edit_result_key(0);
            app.reconcile_paged_prefetch_window(19);
            assert_eq!(app.fs_upload_backlog_contains(0), !warm_gpu);
            app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
            assert_eq!(app.final_composite_cache.contains_key(&cover), warm_gpu);
            if warm_gpu {
                assert_eq!(app.current_edit_result_key(0), before_input);
                assert!(!app.fs_cache.contains_key(&0));
            } else {
                assert!(app.fs_cache.contains_key(&0));
            }
            assert!(app.fs_pending.is_empty());
            assert!(app.retained_cross_session_cache_entries() <= max_entries);
            assert!(app.retained_cross_session_cache_bytes() <= 1024 * 1024);
        }
    }
}

#[test]
fn paged_prefetch_missing_cover_uses_existing_retained_source_queue_and_paced_poll() {
    let ctx = egui::Context::default();
    let mut app = pdf_book(&ctx);
    app.fs_cache.remove(&0);
    prepare_retained_cover_raster(&mut app, egui::Color32::LIGHT_BLUE);
    assert!(app.fs_page_load_state(0).needs_load_request());
    app.reconcile_paged_prefetch_window(19);
    assert!(app.fs_upload_backlog_contains(0));
    assert!(!app.fs_cache.contains_key(&0));
    assert!(app.fs_pending.is_empty());
    app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
    assert!(app.fs_cache.contains_key(&0));
    assert!(!app.fs_upload_backlog_contains(0));
    assert!(app.final_ai_pending.is_empty());
}

#[test]
fn paged_prefetch_reconciles_old_raw_demand_after_the_displayed_unit_changes() {
    let ctx = egui::Context::default();
    let mut app = pdf_book(&ctx);
    app.items[10] = GridItem::Image(app.tmp.path().join("old-partner.nef"));
    app.viewer_navigation_caches.invalidate();
    app.update_prefetch_window(19);
    assert!(!app.fs_cache.contains_key(&10));
    assert!(app.raw_pages.demand.is_empty());
    app.raw_pages
        .demand
        .insert(10, crate::raw::RawPriority::High);
    app.reconcile_paged_prefetch_window(19);
    assert!(app.raw_pages.demand.is_empty());
    assert!(app.fs_pending.is_empty());
}

#[test]
fn paged_prefetch_gpu_ready_latest_seek_supersedes_only_its_own_waiting_sources() {
    let ctx = egui::Context::default();
    let mut app = pdf_book(&ctx);
    let cover = retain_pdf_ai(&mut app, 0);
    app.prefetch_retained_paged_composites(&ctx, 19);
    complete_pending_composites(&mut app, &ctx);
    let owner = app.fs_page_load_context_serial();
    let old = app.fs_page_load_scheduler.request(
        owner,
        10,
        FsPageLoadPriority::Normal,
        FsPageLoadContract::Sequential,
        None,
        0,
    );
    let sibling = app.fs_page_load_scheduler.request(
        owner.wrapping_add(1),
        10,
        FsPageLoadPriority::Normal,
        FsPageLoadContract::Sequential,
        None,
        0,
    );
    let old_cancel = old.cancel_token();
    let sibling_cancel = sibling.cancel_token();
    assert_eq!(
        app.ensure_fs_display_page_load_with_contract(0, FsPageLoadContract::LatestSeek),
        FsPageLoadState::DisplayReady(FsPageDisplaySource::ResidentFinalComposite)
    );
    assert!(old_cancel.load(Ordering::Relaxed));
    assert!(!sibling_cancel.load(Ordering::Relaxed));
    assert!(app.final_composite_cache.contains_key(&cover));
    assert!(app.fs_pending.is_empty());
}

#[cfg(windows)]
#[test]
fn paged_prefetch_reconcile_warm_and_poll_preserve_sibling_cache_and_channels() {
    let ctx = egui::Context::default();
    let mut app = pdf_book(&ctx);
    let main_cover = retain_pdf_ai(&mut app, 0);
    let items = app.items.clone();
    let folder = app.tmp.path().join("sibling-prefetch.pdf");
    let (source_tx, source_rx) = mpsc::channel();
    let (effect_tx, effect_rx) = mpsc::channel();
    let source_cancel = Arc::new(AtomicBool::new(false));
    let effect_cancel = Arc::new(AtomicBool::new(false));
    let source_cancel_for_owner = Arc::clone(&source_cancel);
    let effect_cancel_for_owner = Arc::clone(&effect_cancel);
    let sibling = app.build_window_context_for_test(29_341, |owner| {
        owner.items = items;
        for item in &mut owner.items {
            if let GridItem::PdfPage { pdf_path, .. } = item {
                *pdf_path = folder.clone();
            }
        }
        owner.current_folder = Some(folder);
        owner
            .top_level_grid_view
            .replace_surface(top_level_grid_view::TopLevelGridSurface::Folder);
        owner.visible_indices = (0..22).collect();
        owner.details_order = (0..22).collect();
        owner.fullscreen_idx = Some(19);
        owner.spread_mode = crate::settings::SpreadMode::LtrCover;
        owner.final_cover_spread_preference = crate::settings::FinalCoverSpreadPreference::On;
        owner.viewer_navigation_caches.invalidate();
        for idx in 0..22 {
            let pixels = Arc::new(egui::ColorImage::filled([4, 6], egui::Color32::LIGHT_GREEN));
            let tex = ctx.load_texture(
                format!("sibling-prefetch-{idx}"),
                Arc::clone(&pixels),
                DISPLAY_IMAGE_TEXTURE_OPTIONS,
            );
            owner.fs_cache.insert(
                idx,
                FsCacheEntry::Static {
                    tex,
                    pixels,
                    source_dims: Some([4, 6]),
                    load_seq: 20,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
        }
        owner.fs_cache.remove(&21);
        owner.fs_pending.insert(
            21,
            FsPendingValue::new(
                source_cancel_for_owner,
                source_rx,
                20,
                FsLoadPurpose::for_page(false),
            ),
        );
        let params = owner.effective_params(0);
        let effect_key =
            owner.final_composite_key_for_pixels(owner.current_edit_result_key(0), [4, 6], params);
        let generation = owner.items_generation;
        owner.final_effect_pending.insert(
            effect_key,
            FinalEffectPending {
                source_size: [4, 6],
                cancel: effect_cancel_for_owner,
                rx: effect_rx,
                items_generation: generation,
                output_complete: true,
                nearest_sampler: false,
                prefetch: true,
            },
        );
    });
    let sibling_before = app.with_window_context_for_test(sibling, |owner| {
        (
            owner.items_generation,
            owner.current_edit_result_key(0),
            static_texture_id(owner, 0),
            owner.fs_cache.len(),
        )
    });
    source_tx
        .send(FsLoadResult::StaticCached {
            pixels: Arc::new(egui::ColorImage::filled([4, 6], egui::Color32::LIGHT_GREEN)),
            source_dims: [4, 6],
        })
        .unwrap();
    effect_tx
        .send(FinalCompositeResult::Ready {
            pixels: Arc::new(egui::ColorImage::filled(
                [8, 12],
                egui::Color32::LIGHT_GREEN,
            )),
            elapsed_ms: 0.0,
            timing: FinalCompositeTiming::default(),
        })
        .unwrap();
    app.reconcile_paged_prefetch_window(19);
    app.prefetch_retained_paged_composites(&ctx, 19);
    complete_pending_composites(&mut app, &ctx);
    let main_texture = app
        .final_composite_cache
        .get(&main_cover)
        .unwrap()
        .texture
        .id();
    assert!(!source_cancel.load(Ordering::Relaxed));
    assert!(!effect_cancel.load(Ordering::Relaxed));
    app.with_window_context_for_test(sibling, |owner| {
        assert_eq!(
            (
                owner.items_generation,
                owner.current_edit_result_key(0),
                static_texture_id(owner, 0),
                owner.fs_cache.len()
            ),
            sibling_before
        );
        assert!(Arc::ptr_eq(
            &owner.fs_pending.get(&21).unwrap().cancel,
            &source_cancel
        ));
        assert_eq!(owner.final_effect_pending.len(), 1);
        assert!(owner.final_composite_cache.is_empty());
        owner.reconcile_paged_prefetch_window(19);
        owner.prefetch_retained_paged_composites(&ctx, 19);
        owner.poll_final_effects(&ctx);
        owner.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
        assert!(owner.final_effect_pending.is_empty());
        assert!(owner.fs_pending.is_empty());
        assert!(
            owner
                .final_composite_cache
                .values()
                .any(|entry| entry.complete)
        );
        assert!(owner.fs_cache.contains_key(&21));
    });
    assert_eq!(
        app.final_composite_cache
            .get(&main_cover)
            .unwrap()
            .texture
            .id(),
        main_texture
    );
    assert!(app.final_effect_pending.is_empty());
    assert!(app.fs_pending.is_empty());
}
