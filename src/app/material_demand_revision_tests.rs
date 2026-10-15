#![cfg(test)]

use super::composite_pixels_revision_tests::{evict_pair_cpu, warm_final_pair};
use super::paged_prefetch_tests::{complete_pending_composites, pdf_book, retain_pdf_ai};
use super::*;
use crate::page_dims::{CanonicalPageDimensions, PagePixelDimensions};

fn single_neighbor(app: &mut App) {
    app.fullscreen_idx = Some(1);
    app.spread_mode = crate::settings::SpreadMode::Single;
    app.settings.page_turn_animation_enabled = false;
    app.settings.prefetch_forward = 0;
    app.settings.prefetch_back = 0;
    app.settings.ai_upscale_prefetch_forward = 0;
    app.settings.ai_upscale_prefetch_back = 1;
    app.viewer_navigation_caches.invalidate();
    assert_eq!(app.ai_prefetch_targets(1), vec![0]);
}

fn completed_neighbor(ctx: &egui::Context) -> (AppTestEnvForTest, FinalCompositeKey) {
    let (mut app, keys) = warm_final_pair(ctx);
    evict_pair_cpu(&mut app);
    single_neighbor(&mut app);
    let key = keys[1];
    app.final_composite_cache
        .get_mut(&key)
        .unwrap()
        .canonical_dims = Some(CanonicalPageDimensions::new(1200, 1800));
    assert_eq!(app.final_composite_cache[&key].source_size, [4, 6]);
    assert_eq!(app.final_composite_cache[&key].pixels.size, [8, 12]);
    assert!(app.current_raw_source_pixels(0).is_none());
    (app, key)
}

fn assert_no_source_demand(app: &App) {
    assert!(
        app.fs_pending.is_empty(),
        "prefetch must not hydrate originals"
    );
    assert!(app.fs_upload_backlog.is_empty());
    assert!(app.current_raw_source_pixels(0).is_none());
    assert!(app.edit_result_cache.is_empty());
    assert!(app.local_adjust_pending.is_empty());
}

#[test]
fn material_demand_revision_ai_effectively_off_does_not_hydrate_neighbor() {
    for globally_disabled in [true, false] {
        let ctx = egui::Context::default();
        let (mut app, key) = completed_neighbor(&ctx);
        app.final_composite_cache.get_mut(&key).unwrap().complete = false;
        if globally_disabled {
            app.settings.ai_feature_mode = crate::settings::AiFeatureMode::Disabled;
        } else {
            let mut params = app.effective_params(0).clone();
            params.upscale_model = None;
            params.denoise_model = None;
            app.adjustment_page_params.insert(0, params);
        }
        let params = app.effective_params(0);
        assert!(app.effective_upscale_request(params).is_none());
        assert!(app.effective_denoise_request(params).is_none());
        app.prefetch_final_ai(&ctx, 1);
        assert_no_source_demand(&app);
        assert!(app.final_ai_cache.is_empty());
        assert!(app.final_ai_pending.is_empty());
        assert!(app.final_effect_pending.is_empty());
    }
}

#[test]
fn material_demand_revision_completed_neighbor_keeps_sole_pixels() {
    let ctx = egui::Context::default();
    let (mut app, key) = completed_neighbor(&ctx);
    let pixels = Arc::clone(&app.final_composite_cache[&key].pixels);
    app.prefetch_final_ai(&ctx, 1);
    assert_no_source_demand(&app);
    assert!(app.final_ai_pending.is_empty());
    assert!(app.final_effect_pending.is_empty());
    assert!(Arc::ptr_eq(
        &pixels,
        &app.final_composite_cache[&key].pixels
    ));
    assert!(app.final_composite_cache[&key].complete);
}

#[test]
fn material_demand_revision_matching_retained_pdf_restores_ai_without_original() {
    let ctx = egui::Context::default();
    let mut app = pdf_book(&ctx);
    let final_key = retain_pdf_ai(&mut app, 0);
    single_neighbor(&mut app);
    let params = app.effective_params(0).clone();
    let ai_key = app
        .final_ai_key_for_pixels(final_key.edit_key, [4, 6], &params)
        .unwrap();
    assert!(app.final_composite_cache.is_empty());
    app.prefetch_final_ai(&ctx, 1);
    assert_no_source_demand(&app);
    assert_eq!(app.final_ai_cache[&ai_key].source_size, [4, 6]);
    assert_eq!(app.final_ai_cache[&ai_key].pixels.size, [8, 12]);
    assert!(app.final_ai_pending.is_empty());
}

#[test]
fn material_demand_revision_matching_retained_image_restores_ai_without_original() {
    let ctx = egui::Context::default();
    let mut app = setup_app_for_test();
    let image_path = app.tmp.path().join("retained-ai-neighbor.png");
    let current_path = app.tmp.path().join("retained-ai-current.png");
    app.items.push(GridItem::Image(image_path));
    app.items.push(GridItem::Image(current_path));
    app.visible_indices = vec![0, 1];
    app.details_order = vec![0, 1];
    app.settings.ai_feature_mode = crate::settings::AiFeatureMode::HighQuality;
    app.settings.global_preset.upscale_model = Some(
        crate::ai::ModelKind::UpscaleRealEsrGeneralV3
            .as_str()
            .into(),
    );
    single_neighbor(&mut app);
    let params = app.effective_params(0).clone();
    let key = app
        .final_ai_key_for_pixels(app.current_edit_result_key(0), [4, 6], &params)
        .unwrap();
    let pixels = Arc::new(egui::ColorImage::filled([8, 12], egui::Color32::WHITE));
    assert!(app.insert_retained_final_ai(0, key, [4, 6], Arc::clone(&pixels)));
    app.prefetch_final_ai(&ctx, 1);
    assert_no_source_demand(&app);
    assert!(Arc::ptr_eq(&pixels, &app.final_ai_cache[&key].pixels));
    assert!(app.final_composite_cache.is_empty());
    assert!(app.final_ai_pending.is_empty());
}

#[test]
fn material_demand_revision_existing_ai_outcomes_do_not_hydrate_input() {
    for outcome in ["resident", "cancelled-pending", "failed"] {
        let ctx = egui::Context::default();
        let (mut app, final_key) = completed_neighbor(&ctx);
        app.final_composite_cache
            .get_mut(&final_key)
            .unwrap()
            .complete = false;
        let params = app.effective_params(0).clone();
        let key = app
            .final_ai_key_for_pixels(final_key.edit_key, [4, 6], &params)
            .unwrap();
        match outcome {
            "resident" => {
                app.final_ai_cache.insert(
                    key,
                    FinalAiEntry {
                        canonical_dims: Some(CanonicalPageDimensions::new(1200, 1800)),
                        source_size: [4, 6],
                        pixels: Arc::new(egui::ColorImage::filled([8, 12], egui::Color32::WHITE)),
                        used_upscale: true,
                    },
                );
            }
            "cancelled-pending" => {
                let retained_epoch = app.retained_final_ai_epoch;
                app.final_ai_pending.insert(
                    key,
                    FinalAiPending {
                        job_id: 9001,
                        cancel: Arc::new(AtomicBool::new(true)),
                        retained_key: None,
                        retained_epoch,
                        allow_retained_orphan: false,
                        cancel_on_drop: true,
                    },
                );
            }
            "failed" => {
                app.final_ai_failed.insert(key);
            }
            _ => unreachable!(),
        }
        app.prefetch_final_ai(&ctx, 1);
        assert_no_source_demand(&app);
        assert_eq!(
            app.final_ai_pending.len(),
            usize::from(outcome == "cancelled-pending")
        );
        assert!(app.final_effect_pending.is_empty());
    }
}

#[test]
fn material_demand_revision_ai_size_gate_uses_edit_canvas_not_coordinates_or_output() {
    for limit in [2, 8] {
        let ctx = egui::Context::default();
        let (mut app, key) = completed_neighbor(&ctx);
        let generation = app.items_generation;
        app.page_dims_cache.record(
            generation,
            0,
            crate::page_dims::PagePixelDimensions::Canonical(CanonicalPageDimensions::new(
                1200, 1800,
            )),
        );
        app.final_composite_cache.get_mut(&key).unwrap().complete = false;
        app.settings.ai_upscale_size_limit =
            Some(crate::ai::upscale::AiProcessSizeLimit::square(limit));
        assert_eq!(app.source_dims_for_idx(0), Some((1200.0, 1800.0)));
        let params = app.effective_params(0).clone();
        assert_eq!(app.final_ai_prefetch_input_size(0, &params), Some([4, 6]));
        app.prefetch_final_ai(&ctx, 1);
        if limit == 2 {
            assert_no_source_demand(&app);
        } else {
            assert_eq!(app.fs_pending.len(), 1);
            assert_eq!(
                app.fs_pending.get(&0).unwrap().purpose,
                FsLoadPurpose::Original {
                    entry: OriginalMaterialEntry::MaterialOnly,
                }
            );
        }
        assert!(app.final_ai_pending.is_empty());
    }
}

#[test]
fn material_demand_revision_zero_ai_window_does_not_hydrate_single_neighbor() {
    let ctx = egui::Context::default();
    let (mut app, key) = completed_neighbor(&ctx);
    app.final_composite_cache.get_mut(&key).unwrap().complete = false;
    app.settings.ai_upscale_prefetch_back = 0;
    assert!(app.ai_prefetch_targets(1).is_empty());
    app.prefetch_final_ai(&ctx, 1);
    assert_no_source_demand(&app);
    assert!(app.final_ai_pending.is_empty());
}

#[test]
fn material_demand_revision_zero_ai_window_preserves_displayed_spread_partner() {
    let ctx = egui::Context::default();
    let mut app = pdf_book(&ctx);
    app.settings.ai_upscale_prefetch_forward = 0;
    app.settings.ai_upscale_prefetch_back = 0;
    assert_eq!(app.displayed_spread_partner(19), Some(20));
    assert!(app.ai_prefetch_targets(19).contains(&20));
    let key = retain_pdf_ai(&mut app, 20);
    app.prefetch_final_ai(&ctx, 19);
    assert!(!app.fs_pending.contains_key(&20));
    assert!(!app.fs_upload_backlog_contains(20));
    assert!(
        app.final_ai_cache
            .keys()
            .any(|ai| ai.edit_key == key.edit_key)
    );
}

#[test]
fn material_demand_revision_completed_colorize_and_lut_effects_do_not_hydrate() {
    for lut in [false, true] {
        let ctx = egui::Context::default();
        let mut app = pdf_book(&ctx);
        app.settings.ai_feature_mode = crate::settings::AiFeatureMode::HighQuality;
        app.settings.global_preset.upscale_model = Some(
            crate::ai::ModelKind::UpscaleRealEsrGeneralV3
                .as_str()
                .into(),
        );
        if lut {
            let entries = app.settings.creative_luts.clone();
            let id = entries.first().unwrap().id;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while app.creative_lut_library.get(Some(id)).is_none() {
                app.creative_lut_library.poll(&entries, &ctx);
                assert!(
                    std::time::Instant::now() < deadline,
                    "builtin LUT did not load"
                );
                std::thread::yield_now();
            }
            let mut params = app.effective_params(0).clone();
            params.creative_lut.id = Some(id);
            params.creative_lut.strength = 1.0;
            app.adjustment_page_params.insert(0, params);
        } else {
            app.settings.global_preset.colorize.mode = crate::colorize::ColorizeMode::AllImages;
        }
        let key = retain_pdf_ai(&mut app, 0);
        app.prefetch_retained_paged_composites(&ctx, 19);
        assert!(app.final_effect_pending.contains_key(&key));
        complete_pending_composites(&mut app, &ctx);
        assert!(app.final_composite_cache[&key].complete);
        evict_pair_cpu(&mut app);
        single_neighbor(&mut app);
        let pixels = Arc::clone(&app.final_composite_cache[&key].pixels);
        app.prefetch_final_effects(&ctx, 1, None);
        app.prefetch_final_ai(&ctx, 1);
        assert_no_source_demand(&app);
        assert!(app.final_effect_pending.is_empty());
        assert!(app.final_ai_pending.is_empty());
        assert!(Arc::ptr_eq(
            &pixels,
            &app.final_composite_cache[&key].pixels
        ));
        assert!(app.final_composite_cache[&key].complete);
    }
}

#[test]
fn material_demand_revision_ready_local_stage_and_inactive_layers_need_no_original_for_export() {
    for active in [false, true] {
        let ctx = egui::Context::default();
        let (mut app, _) = completed_neighbor(&ctx);
        app.fullscreen_idx = Some(0);
        app.final_composite_cache.clear();
        let generation = app.items_generation;
        app.page_dims_cache.record(
            generation,
            0,
            PagePixelDimensions::Canonical(CanonicalPageDimensions::new(1200, 1800)),
        );
        let layers = if active {
            vec![local_adjust_core::LocalAdjustmentLayer::new(
                "ready",
                local_adjust_core::LocalMask::Full,
                local_adjust_core::LocalEffect::None,
            )]
        } else {
            Vec::new()
        };
        app.local_adjust_page_layers.insert(0, Arc::new(layers));
        app.local_adjust_pages.insert(0);
        if active {
            let pixels = Arc::new(egui::ColorImage::filled([4, 6], egui::Color32::WHITE));
            let texture = ctx.load_texture(
                "ready-local-stage",
                Arc::clone(&pixels),
                DISPLAY_IMAGE_TEXTURE_OPTIONS,
            );
            let key = app.current_local_adjust_key(0);
            app.local_adjust_cache
                .insert(key, LocalAdjustCacheEntry { pixels, texture });
        }
        app.settings.ai_feature_mode = crate::settings::AiFeatureMode::Disabled;
        app.settings.global_preset.upscale_model = None;
        app.comic_font_registry_loaded = true;
        app.comic_fonts_loaded = true;
        app.comic_fonts = Some(Arc::new(comic_core::FontSet::new()));
        app.editing_addon_declined_session = true;
        app.show_whats_new = false;
        app.show_mouse_nav_migration_prompt = false;
        app.network_data_dir_notice = None;
        app.maybe_start_local_adjust_render(0);
        assert_no_source_demand(&app);
        app.open_export_dialog_for_current(&ctx, 0);
        assert!(
            app.export_dialog.is_some(),
            "Ctrl+E must retain the prepared-stage contract"
        );
        assert_no_source_demand(&app);
        assert!(app.final_composite_cache.is_empty());
    }
}

fn configure_final_effect(app: &mut App, ctx: &egui::Context, lut: bool) {
    app.settings.ai_feature_mode = crate::settings::AiFeatureMode::HighQuality;
    app.settings.global_preset.upscale_model = Some(
        crate::ai::ModelKind::UpscaleRealEsrGeneralV3
            .as_str()
            .into(),
    );
    if lut {
        let entries = app.settings.creative_luts.clone();
        let id = entries.first().unwrap().id;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.creative_lut_library.get(Some(id)).is_none() {
            app.creative_lut_library.poll(&entries, ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "builtin LUT did not load"
            );
            std::thread::yield_now();
        }
        let mut params = app.effective_params(0).clone();
        params.creative_lut.id = Some(id);
        params.creative_lut.strength = 1.0;
        app.adjustment_page_params.insert(0, params);
    } else {
        app.settings.global_preset.colorize.mode = crate::colorize::ColorizeMode::AllImages;
    }
}

#[test]
fn material_demand_revision_native_pdf_ai_target_precedes_fit_size_rejection() {
    let ctx = egui::Context::default();
    let mut app = pdf_book(&ctx);
    single_neighbor(&mut app);
    app.settings.ai_feature_mode = crate::settings::AiFeatureMode::HighQuality;
    app.settings.global_preset.upscale_model = Some(
        crate::ai::ModelKind::UpscaleRealEsrGeneralV3
            .as_str()
            .into(),
    );
    app.settings.ai_upscale_size_limit = Some(crate::ai::upscale::AiProcessSizeLimit::square(1024));
    let GridItem::PdfPage { content_type, .. } = &mut app.items[0] else {
        panic!("PDF fixture");
    };
    *content_type = Some(crate::pdf_loader::PdfPageContentType::Raster { w: 400, h: 600 });
    let pixels = Arc::new(egui::ColorImage::filled([1200, 1800], egui::Color32::WHITE));
    let texture = ctx.load_texture(
        "oversized-fit-ai",
        Arc::clone(&pixels),
        DISPLAY_IMAGE_TEXTURE_OPTIONS,
    );
    app.fs_cache.insert(
        0,
        FsCacheEntry::Static {
            tex: texture,
            pixels,
            source_dims: Some([1200, 1800]),
            load_seq: 1,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    let params = app.effective_params(0).clone();
    let edit_key = app.current_edit_result_key(0);
    assert!(
        app.final_ai_key_for_pixels(edit_key, [1200, 1800], &params)
            .is_none()
    );
    assert!(
        app.final_ai_key_for_pixels(edit_key, [400, 600], &params)
            .is_some()
    );
    assert!(app.pdf_prefetch_should_defer_ai(0));
    let probe = PdfRerenderRequestProbe::start();
    app.prefetch_final_ai(&ctx, 1);
    assert_eq!(probe.take_requests(), vec![(0, 600)]);
    assert!(app.final_ai_pending.is_empty());
    assert!(app.edit_result_cache.is_empty());
    assert!(app.fs_upload_backlog.is_empty());
}

#[test]
fn material_demand_revision_cached_ai_supplies_effect_without_edit_hydration() {
    for lut in [false, true] {
        let ctx = egui::Context::default();
        let mut app = pdf_book(&ctx);
        configure_final_effect(&mut app, &ctx, lut);
        single_neighbor(&mut app);
        app.fs_cache.remove(&0);
        let params = app.effective_params(0).clone();
        let edit_key = app.current_edit_result_key(0);
        let ai_key = app
            .final_ai_key_for_pixels(edit_key, [4, 6], &params)
            .unwrap();
        let pixels = Arc::new(egui::ColorImage::filled([8, 12], egui::Color32::WHITE));
        app.final_ai_cache.insert(
            ai_key,
            FinalAiEntry {
                canonical_dims: Some(CanonicalPageDimensions::new(1200, 1800)),
                source_size: [4, 6],
                pixels: Arc::clone(&pixels),
                used_upscale: true,
            },
        );
        let final_key = app.final_composite_key_for_pixels(edit_key, [4, 6], &params);
        assert!(app.final_composite_cache.is_empty());
        app.prefetch_final_effects(&ctx, 1, None);
        assert_no_source_demand(&app);
        assert_eq!(app.final_effect_pending.len(), 1);
        assert_eq!(app.final_effect_pending[&final_key].source_size, [4, 6]);
        assert!(Arc::ptr_eq(&pixels, &app.final_ai_cache[&ai_key].pixels));
        complete_pending_composites(&mut app, &ctx);
        let completed = &app.final_composite_cache[&final_key];
        assert!(completed.complete);
        assert_eq!(completed.source_size, [4, 6]);
        assert_eq!(completed.pixels.size, [8, 12]);
        assert_no_source_demand(&app);
        assert!(app.final_ai_pending.is_empty());
    }
}

#[test]
fn material_demand_revision_retained_display_waits_for_effect_without_original_fallthrough() {
    for lut in [false, true] {
        let ctx = egui::Context::default();
        let mut app = pdf_book(&ctx);
        configure_final_effect(&mut app, &ctx, lut);
        let final_key = retain_pdf_ai(&mut app, 0);
        app.fullscreen_idx = Some(0);
        app.spread_mode = crate::settings::SpreadMode::Single;
        app.settings.page_turn_animation_enabled = false;
        app.viewer_navigation_caches.invalidate();
        assert!(app.final_composite_cache.is_empty());
        assert!(app.ensure_final_composite_texture(&ctx, 0).is_none());
        assert_no_source_demand(&app);
        assert_eq!(app.final_effect_pending.len(), 1);
        assert!(app.final_effect_pending.contains_key(&final_key));
        complete_pending_composites(&mut app, &ctx);
        let completed = &app.final_composite_cache[&final_key];
        assert!(completed.complete);
        let texture_id = completed.texture.id();
        assert_eq!(
            app.ensure_final_composite_texture(&ctx, 0).unwrap().id(),
            texture_id
        );
        assert_no_source_demand(&app);
        assert!(app.final_ai_pending.is_empty());
    }
}

#[test]
#[cfg(windows)]
fn material_demand_revision_initial_materializer_cancel_is_scoped_to_mounted_context() {
    for local in [false, true] {
        let ctx = egui::Context::default();
        let (mut app, final_key) = completed_neighbor(&ctx);
        app.fullscreen_idx = Some(0);
        let main_context = app.projected_viewer_context_id();
        let item = app.items[0].clone();
        let inject = |owner: &mut App| {
            let (tx, rx) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            let request = EditMaterializeRequest {
                items_generation: owner.items_generation,
                input_seq: owner.input_seq,
                kind: if local {
                    EditMaterializeKind::Local {
                        key: owner.current_local_adjust_key(0),
                        continuation: LocalMaterializeContinuation::EnterConceal {
                            requested_fs_idx: 0,
                        },
                    }
                } else {
                    EditMaterializeKind::Conceal {
                        key: owner.current_edit_result_key(0),
                        purpose: ConcealMaterializePurpose::EnterMode {
                            requested_fs_idx: 0,
                        },
                    }
                },
            };
            owner.local_adjust_pending.insert(
                0,
                LocalAdjustRenderPending {
                    request,
                    cancel: Arc::clone(&cancel),
                    rx,
                    ready: None,
                },
            );
            (tx, cancel, request)
        };
        let mut sibling_worker = None;
        let sibling_context = app
            .build_viewer_context("initial-materializer-cancel-sibling", |owner, id| {
                assert_ne!(id, main_context);
                owner.items.push(item);
                owner.visible_indices = vec![0];
                owner.details_order = vec![0];
                owner.fullscreen_idx = Some(0);
                owner.spread_mode = crate::settings::SpreadMode::Single;
                sibling_worker = Some(inject(owner));
                BuildOutcome::Commit
            })
            .unwrap();
        let (_main_tx, main_cancel, _) = inject(&mut app);
        let (_sibling_tx, sibling_cancel, sibling_request) = sibling_worker.unwrap();
        let pixels = Arc::clone(&app.final_composite_cache[&final_key].pixels);
        app.cancel_original_material_entry_intents(None);
        assert!(main_cancel.load(Ordering::Relaxed));
        assert!(app.local_adjust_pending.is_empty());
        assert!(!sibling_cancel.load(Ordering::Relaxed));
        app.with_viewer_context(sibling_context, |owner| {
            assert_eq!(owner.projected_viewer_context_id(), sibling_context);
            assert_eq!(owner.local_adjust_pending.len(), 1);
            let pending = &owner.local_adjust_pending[&0];
            assert_eq!(pending.request, sibling_request);
            assert!(Arc::ptr_eq(&pending.cancel, &sibling_cancel));
            owner.cancel_original_material_entry_intents(None);
            assert!(sibling_cancel.load(Ordering::Relaxed));
            assert!(owner.local_adjust_pending.is_empty());
        })
        .unwrap();
        assert_eq!(app.projected_viewer_context_id(), main_context);
        assert!(app.local_adjust_pending.is_empty());
        assert!(Arc::ptr_eq(
            &pixels,
            &app.final_composite_cache[&final_key].pixels
        ));
        assert!(app.fs_pending.is_empty());
    }
}
