#![cfg(test)]

use super::composite_pixels_revision_tests::{
    complete_comic, evict_pair_cpu, seed_annotation, warm_final_pair,
};
use super::*;
use crate::page_dims::{CanonicalPageDimensions, PagePixelDimensions};

fn completed_only(ctx: &egui::Context) -> (AppTestEnvForTest, FinalCompositeKey) {
    let (mut app, keys) = warm_final_pair(ctx);
    evict_pair_cpu(&mut app);
    app.fullscreen_idx = Some(0);
    app.spread_mode = crate::settings::SpreadMode::Single;
    app.settings.page_turn_animation_enabled = false;
    app.viewer_navigation_caches.invalidate();
    let key = keys[1];
    app.final_composite_cache
        .get_mut(&key)
        .unwrap()
        .canonical_dims = Some(CanonicalPageDimensions::new(1200, 1800));
    app.page_dims_cache.clear();
    assert!(!app.fs_cache.contains_key(&0));
    (app, key)
}

fn inject_pending(app: &mut App, entry: OriginalMaterialEntry) -> mpsc::Sender<FsLoadResult> {
    let (tx, rx) = mpsc::channel();
    app.fs_pending.insert(
        0,
        FsPendingValue::new(
            Arc::new(AtomicBool::new(false)),
            rx,
            app.input_seq,
            FsLoadPurpose::Original { entry },
        ),
    );
    tx
}

fn original_result() -> FsLoadResult {
    FsLoadResult::Static {
        ci: egui::ColorImage::filled([4, 6], egui::Color32::BLUE),
        source_dims: [1200, 1800],
        animation: StaticAnimationState::Still,
    }
}

#[test]
fn original_material_producer_captures_unclamped_basis_through_retained_and_final_worker() {
    use super::paged_prefetch_tests::{complete_pending_composites, pdf_book, retain_pdf_ai};
    let ctx = egui::Context::default();
    let mut app = pdf_book(&ctx);
    if let Some(FsCacheEntry::Static { source_dims, .. }) = app.fs_cache.get_mut(&0) {
        *source_dims = Some([1200, 1800]);
    }
    let generation = app.items_generation;
    app.page_dims_cache
        .record(generation, 0, PagePixelDimensions::Thumbnail((160, 240)));
    let key = retain_pdf_ai(&mut app, 0);
    assert_eq!(app.source_dims_for_idx(0), Some((1200.0, 1800.0)));
    app.fullscreen_idx = Some(0);
    app.spread_mode = crate::settings::SpreadMode::Single;
    app.settings.page_turn_animation_enabled = false;
    app.viewer_navigation_caches.invalidate();
    app.ensure_final_composite_texture(&ctx, 0);
    complete_pending_composites(&mut app, &ctx);
    assert_eq!(
        app.final_composite_cache[&key].canonical_dims,
        Some(CanonicalPageDimensions::new(1200, 1800))
    );
    app.settings.retained_final_ai_cache_max_entries = 1;
    retain_pdf_ai(&mut app, 10);
    app.final_ai_cache.clear();
    app.page_dims_cache.clear();
    app.page_dims_cache
        .record(generation, 0, PagePixelDimensions::Thumbnail((160, 240)));
    assert_eq!(app.source_dims_for_idx(0), Some((1200.0, 1800.0)));
    assert!(app.current_raw_source_pixels(0).is_none());
    seed_annotation(&mut app, [1200, 1800]);
    let exported = app.comic_composited_pixels_for_export(&ctx, 0).unwrap();
    assert_eq!(exported[(4, 6)], egui::Color32::RED);
    assert_eq!(exported[(0, 0)], egui::Color32::WHITE);
    assert!(app.ensure_comic_composite_texture(&ctx, 0).is_none());
    let displayed = complete_comic(&mut app, &ctx);
    assert_eq!(displayed.pixels, exported.pixels);
    let work = app.prepare_capture_pixel_work_for_test(&ctx, 0).unwrap();
    let crate::capture::CapturePixelWork::Single(job) = work else {
        panic!("single annotation capture");
    };
    assert_eq!(job.source.pixels, exported.pixels);
    assert!(app.fs_pending.is_empty() && app.fs_upload_backlog.is_empty());
    assert!(app.current_raw_source_pixels(0).is_none());
}

#[test]
fn original_material_canonical_basis_survives_cpu_eviction_and_pdf_thumbnail() {
    let ctx = egui::Context::default();
    let (mut app, key) = completed_only(&ctx);
    let generation = app.items_generation;
    app.page_dims_cache
        .record(generation, 0, PagePixelDimensions::Thumbnail((160, 240)));
    assert_eq!(app.source_dims_for_idx(0), Some((1200.0, 1800.0)));
    assert_eq!(app.final_composite_cache[&key].source_size, [4, 6]);
    assert_eq!(app.final_composite_cache[&key].pixels.size, [8, 12]);
    assert_eq!(
        app.fs_page_load_state(0),
        FsPageLoadState::DisplayReady(FsPageDisplaySource::ResidentFinalComposite)
    );
    let pixels = Arc::clone(&app.final_composite_cache[&key].pixels);
    let (_, actual) = app.ensure_final_composite_pixels_with_key(&ctx, 0).unwrap();
    assert!(Arc::ptr_eq(&pixels, &actual));
    assert!(app.fs_pending.is_empty() && app.fs_upload_backlog.is_empty());
}

#[test]
fn original_material_cold_admission_bypasses_retained_display_shortcut_without_second_owner() {
    for evict_retained in [false, true] {
        let ctx = egui::Context::default();
        let (mut app, keys) = warm_final_pair(&ctx);
        if evict_retained {
            evict_pair_cpu(&mut app);
        }
        app.fullscreen_idx = Some(0);
        app.spread_mode = crate::settings::SpreadMode::Single;
        app.viewer_navigation_caches.invalidate();
        app.fs_pdf_display_target = None;
        assert!(app.exact_complete_final_pixels_available(0));
        assert_eq!(
            app.has_retained_pdf_final_ai_for_current_params(0),
            !evict_retained
        );
        assert!(app.fs_pending.is_empty() && app.fs_upload_backlog.is_empty());
        assert!(!app.ensure_original_material_for_entry(0, OriginalMaterialEntry::MaterialOnly));
        let pending = app
            .fs_pending
            .get(&0)
            .expect("cold original demand must enqueue the existing source loader");
        assert_eq!(
            pending.purpose,
            FsLoadPurpose::Original {
                entry: OriginalMaterialEntry::MaterialOnly
            }
        );
        let cancellation = Arc::clone(&pending.cancel);
        assert!(!app.ensure_original_material_for_entry(0, OriginalMaterialEntry::MaterialOnly));
        assert_eq!(app.fs_pending.len(), 1);
        assert!(Arc::ptr_eq(
            &cancellation,
            &app.fs_pending.get(&0).unwrap().cancel
        ));
        assert!(app.final_composite_cache.contains_key(&keys[1]));
        assert!(app.current_raw_source_pixels(0).is_none());
        app.fs_pending.remove(&0).unwrap().cancel();
    }
}

#[test]
fn original_material_success_and_failure_preserve_completed_pixels_and_keys() {
    for success in [true, false] {
        let ctx = egui::Context::default();
        let (mut app, key) = completed_only(&ctx);
        let before = app.current_edit_result_key(0);
        let pixels = Arc::clone(&app.final_composite_cache[&key].pixels);
        let texture = app.final_composite_cache[&key].texture.id();
        let tx = inject_pending(&mut app, OriginalMaterialEntry::MaterialOnly);
        tx.send(if success {
            original_result()
        } else {
            FsLoadResult::Failed
        })
        .unwrap();
        app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
        assert_eq!(app.current_edit_result_key(0), before);
        let completed = &app.final_composite_cache[&key];
        assert!(Arc::ptr_eq(&pixels, &completed.pixels));
        assert_eq!(completed.texture.id(), texture);
        assert_eq!(app.current_raw_source_pixels(0).is_some(), success);
        assert!(app.fs_pending.is_empty() && app.fs_upload_backlog.is_empty());
        assert!(app.exact_complete_final_pixels_available(0));
        if !success {
            assert!(app.fs_feedback_toast.is_some());
            assert!(
                !app.ensure_original_material_for_entry(0, OriginalMaterialEntry::MaterialOnly)
            );
            assert!(app.fs_pending.is_empty() && app.fs_upload_backlog.is_empty());
        }
    }
}

#[test]
fn original_material_joining_normal_pdf_load_preserves_reload_adoption_and_new_coordinates() {
    for backlog in [false, true] {
        let ctx = egui::Context::default();
        let (mut app, old_final_key) = completed_only(&ctx);
        app.show_whats_new = false;
        app.show_mouse_nav_migration_prompt = false;
        app.network_data_dir_notice = None;
        let before = app.current_edit_result_key(0);
        let (tx, rx) = mpsc::channel();
        let changed_raster = FsLoadResult::Static {
            ci: egui::ColorImage::filled([4, 6], egui::Color32::BLUE),
            source_dims: [2000, 3000],
            animation: StaticAnimationState::Still,
        };
        if backlog {
            let generation = app.items_generation;
            app.fs_upload_backlog.push_for_generation(
                generation,
                FsUploadResult::new(0, changed_raster, 7, FsLoadPurpose::Display),
            );
        } else {
            app.fs_pending.insert(
                0,
                FsPendingValue::new(
                    Arc::new(AtomicBool::new(false)),
                    rx,
                    7,
                    FsLoadPurpose::Display,
                ),
            );
            tx.send(changed_raster).unwrap();
        }
        let entry = OriginalMaterialEntry::LocalAdjustment {
            requested_fs_idx: 0,
        };
        assert!(!app.ensure_original_material_for_entry(0, entry));
        let expected = FsLoadPurpose::ResumeAfterNormalLoad {
            entry,
            normal: FsNormalLoadPurpose::Display,
        };
        if backlog {
            assert_eq!(
                app.fs_upload_backlog.iter().next().unwrap().purpose,
                expected
            );
        } else {
            assert_eq!(app.fs_pending.get(&0).unwrap().purpose, expected);
        }
        app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
        assert_ne!(app.current_edit_result_key(0).source_gen, before.source_gen);
        assert!(!app.final_composite_cache.contains_key(&old_final_key));
        assert_eq!(app.source_dims_for_idx(0), Some((2000.0, 3000.0)));
        assert!(
            app.local_adjust_mode,
            "existing entry resumes after normal source adoption"
        );
        assert!(app.fs_pending.is_empty() && app.fs_upload_backlog.is_empty());
    }
}

#[test]
fn original_material_normal_join_and_cancel_preserve_animation_worker_contract() {
    let entry = OriginalMaterialEntry::MaterialOnly;
    for normal in [
        FsNormalLoadPurpose::Display,
        FsNormalLoadPurpose::Prefetch,
        FsNormalLoadPurpose::AnimationPromotion {
            target_idx: 1,
            started_at: Instant::now(),
        },
    ] {
        let purpose = normal.purpose().with_original_entry(entry);
        assert_eq!(
            purpose.animation_policy(),
            normal.purpose().animation_policy()
        );
        assert_eq!(
            purpose.promotion_started_at_for(1),
            normal.purpose().promotion_started_at_for(1)
        );
    }
    let ctx = egui::Context::default();
    let (mut app, _) = completed_only(&ctx);
    let (_tx, rx) = mpsc::channel();
    app.fs_pending.insert(
        0,
        FsPendingValue::new(
            Arc::new(AtomicBool::new(false)),
            rx,
            7,
            FsLoadPurpose::Prefetch,
        ),
    );
    assert!(!app.ensure_original_material_for_entry(
        0,
        OriginalMaterialEntry::Erase {
            requested_fs_idx: 0
        }
    ));
    app.cancel_original_material_entry_intents(None);
    assert_eq!(
        app.fs_pending.get(&0).unwrap().purpose,
        FsLoadPurpose::Prefetch
    );
}

#[test]
fn original_material_read_demand_keeps_operation_and_pdf_display_demand_keeps_owner() {
    let ctx = egui::Context::default();
    let (mut app, _) = completed_only(&ctx);
    let entry = OriginalMaterialEntry::Erase {
        requested_fs_idx: 0,
    };
    let _tx = inject_pending(&mut app, entry);
    let cancellation = Arc::clone(&app.fs_pending.get(&0).unwrap().cancel);
    assert!(!app.ensure_original_material_for_entry(0, OriginalMaterialEntry::MaterialOnly));
    assert_eq!(
        app.fs_pending.get(&0).unwrap().purpose,
        FsLoadPurpose::Original { entry }
    );
    app.request_pdf_rerender_at_target(0, 4096, true);
    app.start_fs_load(0);
    assert_eq!(
        app.fs_pending.get(&0).unwrap().purpose,
        FsLoadPurpose::Original { entry }
    );
    assert!(!cancellation.load(Ordering::Relaxed));
    assert!(Arc::ptr_eq(
        &cancellation,
        &app.fs_pending.get(&0).unwrap().cancel
    ));
    app.request_pdf_absolute_demand(0, 3000.0, true);
    let source = app.metadata_cache_key(0).unwrap();
    assert!(app.fs_pdf_rerender_demand.note_absolute(0, source, 3000));
}

#[test]
fn original_material_disconnect_is_terminal_unless_owner_canceled() {
    for canceled in [false, true] {
        let ctx = egui::Context::default();
        let (mut app, key) = completed_only(&ctx);
        let tx = inject_pending(&mut app, OriginalMaterialEntry::MaterialOnly);
        if canceled {
            app.fs_pending.get(&0).unwrap().cancel();
        }
        drop(tx);
        app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
        assert_eq!(
            matches!(app.fs_cache.get(&0), Some(FsCacheEntry::Failed)),
            !canceled
        );
        assert!(app.final_composite_cache.contains_key(&key));
        assert!(app.fs_pending.is_empty());
        if !canceled {
            assert!(
                !app.ensure_original_material_for_entry(0, OriginalMaterialEntry::MaterialOnly)
            );
            assert!(app.fs_pending.is_empty());
        }
    }
}

#[test]
fn original_material_navigation_and_cancel_retire_continuation_without_mutation() {
    let ctx = egui::Context::default();
    let (mut app, key) = completed_only(&ctx);
    let tx = inject_pending(
        &mut app,
        OriginalMaterialEntry::Erase {
            requested_fs_idx: 0,
        },
    );
    app.cancel_superseded_fs_navigation_display_target(1);
    app.fullscreen_idx = Some(1);
    app.cancel_superseded_fs_navigation_display_target(0);
    app.fullscreen_idx = Some(0);
    assert_eq!(
        app.fs_pending.get(&0).unwrap().purpose,
        FsLoadPurpose::Original {
            entry: OriginalMaterialEntry::MaterialOnly
        }
    );
    tx.send(original_result()).unwrap();
    app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
    assert!(!app.erase_mode);
    assert!(app.final_composite_cache.contains_key(&key));
    app.fs_cache.remove(&0);
    let tx = inject_pending(
        &mut app,
        OriginalMaterialEntry::Conceal {
            requested_fs_idx: 0,
        },
    );
    app.fs_pending.remove(&0).unwrap().cancel();
    drop(tx);
    app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
    assert!(!app.conceal_mode);
    assert!(app.final_composite_cache.contains_key(&key));
}

#[test]
#[cfg(windows)]
fn original_material_backlog_merges_entry_and_preserves_sibling_context() {
    let ctx = egui::Context::default();
    let (mut app, key) = completed_only(&ctx);
    // AppTestEnv owns the process-wide test profile lock. Build the second real viewer
    // context inside this App instead of creating a second lock-owning environment.
    let main_context = app.projected_viewer_context_id();
    let sibling_item = app.items[0].clone();
    let sibling_pixels = Arc::clone(&app.final_composite_cache[&key].pixels);
    let source_size = app.final_composite_cache[&key].source_size;
    let canonical_dims = app.final_composite_cache[&key].canonical_dims;
    let mut sibling_snapshot = None;
    let mut sibling_tx = None;
    let sibling_context = app
        .build_viewer_context("original_material_sibling_fixture", |owner, id| {
            assert_ne!(id, main_context);
            owner.items.push(sibling_item);
            owner.visible_indices = vec![0];
            owner.details_order = vec![0];
            owner.fullscreen_idx = Some(0);
            owner.spread_mode = crate::settings::SpreadMode::Single;
            let sibling_edit = owner.current_edit_result_key(0);
            let sibling_key = owner.final_composite_key_for_pixels(
                sibling_edit,
                source_size,
                owner.effective_params(0),
            );
            let texture = ctx.load_texture(
                "original-material-sibling-final",
                Arc::clone(&sibling_pixels),
                DISPLAY_IMAGE_TEXTURE_OPTIONS,
            );
            owner.bind_display_texture_identity(&texture, sibling_edit.item_id);
            let texture_id = texture.id();
            owner.final_composite_cache.insert(
                sibling_key,
                FinalCompositeEntry {
                    canonical_dims,
                    source_size,
                    pixels: Arc::clone(&sibling_pixels),
                    texture,
                    complete: true,
                },
            );
            assert!(owner.exact_complete_final_pixels_available(0));
            sibling_tx = Some(inject_pending(owner, OriginalMaterialEntry::MaterialOnly));
            let cancellation = Arc::clone(&owner.fs_pending.get(&0).unwrap().cancel);
            sibling_snapshot = Some((sibling_key, sibling_edit, cancellation, texture_id));
            BuildOutcome::Commit
        })
        .unwrap();
    let (sibling_key, sibling_edit, sibling_cancel, sibling_texture) = sibling_snapshot.unwrap();
    assert_eq!(app.projected_viewer_context_id(), main_context);
    let generation = app.items_generation;
    app.fs_upload_backlog.push_for_generation(
        generation,
        FsUploadResult::new(
            0,
            original_result(),
            7,
            FsLoadPurpose::Original {
                entry: OriginalMaterialEntry::MaterialOnly,
            },
        ),
    );
    let requested = OriginalMaterialEntry::LocalAdjustment {
        requested_fs_idx: 0,
    };
    assert!(!app.ensure_original_material_for_entry(0, requested));
    assert_eq!(app.fs_upload_backlog.len(), 1);
    assert!(app.fs_pending.is_empty());
    assert_eq!(
        app.fs_upload_backlog.iter().next().unwrap().purpose,
        FsLoadPurpose::Original { entry: requested }
    );
    app.cancel_original_material_entry_intents(None);
    app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
    assert!(app.final_composite_cache.contains_key(&key));
    app.with_viewer_context(sibling_context, |sibling| {
        assert_eq!(sibling.projected_viewer_context_id(), sibling_context);
        assert_eq!(sibling.current_edit_result_key(0), sibling_edit);
        let completed = &sibling.final_composite_cache[&sibling_key];
        assert!(completed.complete);
        assert_eq!(completed.canonical_dims, canonical_dims);
        assert_eq!(completed.texture.id(), sibling_texture);
        assert!(Arc::ptr_eq(&completed.pixels, &sibling_pixels));
        assert!(Arc::ptr_eq(
            &sibling_cancel,
            &sibling.fs_pending.get(&0).unwrap().cancel,
        ));
        assert_eq!(
            sibling.fs_pending.get(&0).unwrap().purpose,
            FsLoadPurpose::Original {
                entry: OriginalMaterialEntry::MaterialOnly,
            },
        );
        assert!(sibling.current_raw_source_pixels(0).is_none());
        assert!(sibling.fs_upload_backlog.is_empty());
    })
    .unwrap();
    assert!(!sibling_cancel.load(Ordering::Relaxed));
    assert_eq!(app.projected_viewer_context_id(), main_context);
    assert!(app.final_composite_cache.contains_key(&key));
    assert!(app.current_raw_source_pixels(0).is_some());
    drop(sibling_tx);
}

#[test]
fn original_material_metadata_entry_does_not_reload_completed_source() {
    let ctx = egui::Context::default();
    let (mut app, key) = completed_only(&ctx);
    let before = app.current_edit_result_key(0);
    app.start_metadata_load(0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.metadata_pending.is_some() {
        app.poll_metadata_load();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(app.current_edit_result_key(0), before);
    assert!(app.final_composite_cache.contains_key(&key));
    assert!(app.fs_pending.is_empty() && app.fs_upload_backlog.is_empty());
    assert!(app.current_raw_source_pixels(0).is_none());
}

#[test]
fn original_material_saved_erase_stage_rebuild_resumes_one_entry_and_preserves_final() {
    use crate::ui_erase::{EraseInpaintKind, EraseInpaintPendingKey, EraseInpaintPurpose};
    for conceal in [false, true] {
        let ctx = egui::Context::default();
        let (mut app, final_key) = completed_only(&ctx);
        app.show_whats_new = false;
        app.show_mouse_nav_migration_prompt = false;
        app.network_data_dir_notice = None;
        app.ai_runtime_init =
            crate::ai::runtime::AiRuntimeInitOwner::failed_for_test("stage fixture uses diffusion");
        let page_key = app.page_path_key(0).unwrap();
        let mut mask = vec![false; 24];
        mask[9] = true;
        if app.mask_db.is_none() {
            app.mask_db = Some(crate::mask_db::MaskDb::open().unwrap());
        }
        app.mask_db
            .as_ref()
            .unwrap()
            .set(&page_key, &mask, &[], 4, 6)
            .unwrap();
        app.mask_pages.insert(0);
        app.mask_page_keys.insert(page_key);
        seed_annotation(&mut app, [1200, 1800]);
        assert!(app.ensure_comic_composite_texture(&ctx, 0).is_none());
        let comic_pixels = complete_comic(&mut app, &ctx);
        let comic_texture = app.comic_cache.get(&0).unwrap().texture.id();
        let before = app.current_edit_result_key(0);
        let final_pixels = Arc::clone(&app.final_composite_cache[&final_key].pixels);
        let source_tx = inject_pending(&mut app, OriginalMaterialEntry::MaterialOnly);
        if conceal {
            app.enter_conceal_mode(&ctx, 0);
        } else {
            app.enter_local_adjust_mode(&ctx);
        }
        assert!(!app.conceal_mode && !app.local_adjust_mode);
        source_tx.send(original_result()).unwrap();
        app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
        let pending_key = EraseInpaintPendingKey {
            idx: 0,
            kind: EraseInpaintKind::Commit,
        };
        assert!(matches!(
            app.erase_inpaint_pending.get(&pending_key).unwrap().purpose,
            EraseInpaintPurpose::MaterializeForEntry { .. }
        ));
        assert!(app.final_composite_cache.contains_key(&final_key));
        // The current stage will remain sufficient if raw is retired again before completion.
        app.fs_cache.remove(&0);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !app.erase_inpaint_pending.is_empty() || !app.local_adjust_pending.is_empty() {
            app.frame_counter += 1;
            app.poll_erase_inpaint(&ctx);
            app.poll_local_adjust_render(&ctx);
            assert!(
                Instant::now() < deadline,
                "single entry must finish through stage owners"
            );
            std::thread::yield_now();
        }
        assert_eq!(app.conceal_mode, conceal);
        assert_eq!(app.local_adjust_mode, !conceal);
        assert!(app.current_erase_result_pixels(0).is_some());
        assert_eq!(app.current_edit_result_key(0), before);
        assert!(Arc::ptr_eq(
            &app.final_composite_cache[&final_key].pixels,
            &final_pixels
        ));
        assert!(Arc::ptr_eq(
            &app.comic_cache.get(&0).unwrap().pixels,
            &comic_pixels
        ));
        assert_eq!(app.comic_cache.get(&0).unwrap().texture.id(), comic_texture);
        assert!(app.fs_pending.is_empty() && app.fs_upload_backlog.is_empty());
        assert!(app.current_raw_source_pixels(0).is_none());
    }
}

#[test]
fn original_material_stage_join_preserves_commit_contract_and_cancel_consumes_only_intent() {
    use crate::ui_erase::{
        EraseInpaintKind, EraseInpaintPending, EraseInpaintPendingKey, EraseInpaintProgress,
        EraseInpaintPurpose, EraseInpaintResult,
    };
    for mutation in [false, true] {
        let ctx = egui::Context::default();
        let (mut app, final_key) = completed_only(&ctx);
        app.show_whats_new = false;
        app.show_mouse_nav_migration_prompt = false;
        app.network_data_dir_notice = None;
        let source_tx = inject_pending(&mut app, OriginalMaterialEntry::MaterialOnly);
        source_tx.send(original_result()).unwrap();
        app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
        app.mask_pages.insert(0);
        let pending_key = EraseInpaintPendingKey {
            idx: 0,
            kind: EraseInpaintKind::Commit,
        };
        let (tx, rx) = mpsc::channel();
        let (_progress_tx, progress_rx) = mpsc::channel();
        let entry = OriginalMaterialEntry::LocalAdjustment {
            requested_fs_idx: 0,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let pending = EraseInpaintPending {
            idx: 0,
            items_generation: app.items_generation,
            path_key: app.page_path_key(0),
            rx,
            progress_rx,
            progress: EraseInpaintProgress::Preparing,
            cancel: Arc::clone(&cancel),
            started_at: Instant::now(),
            input_generation: app.input_generation.get(&0).copied().unwrap_or(0),
            mask_generation: app.erase_mask_generation.get(&0).copied().unwrap_or(0),
            purpose: if mutation {
                EraseInpaintPurpose::Commit {
                    origin: "fixture-commit",
                }
            } else {
                EraseInpaintPurpose::MaterializeForEntry {
                    entry: OriginalMaterialEntry::MaterialOnly,
                }
            },
        };
        app.erase_inpaint_pending.insert(pending_key, pending);
        app.enter_local_adjust_mode(&ctx);
        assert!(!app.local_adjust_mode);
        let joined = app.erase_inpaint_pending.get(&pending_key).unwrap();
        assert!(Arc::ptr_eq(&joined.cancel, &cancel));
        assert_eq!(joined.purpose.entry(), Some(entry));
        if mutation {
            assert!(matches!(
                joined.purpose,
                EraseInpaintPurpose::ResumeAfterCommit { .. }
            ));
        } else {
            app.cancel_original_material_entry_intents(None);
        }
        tx.send(EraseInpaintResult::Pixels(egui::ColorImage::filled(
            [4, 6],
            egui::Color32::GREEN,
        )))
        .unwrap();
        app.poll_erase_inpaint(&ctx);
        assert_eq!(app.local_adjust_mode, mutation);
        assert_eq!(
            app.final_composite_cache.contains_key(&final_key),
            !mutation
        );
        assert!(app.current_erase_result_pixels(0).is_some());
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(app.erase_inpaint_pending.is_empty());
    }
}

#[test]
fn original_material_parameter_recompose_requests_original_then_reuses_hydration() {
    let ctx = egui::Context::default();
    let (mut app, key) = completed_only(&ctx);
    app.settings.global_preset.brightness = 0.2;
    assert!(!app.exact_complete_final_pixels_available(0));
    let tx = inject_pending(&mut app, OriginalMaterialEntry::MaterialOnly);
    assert!(app.ensure_edit_result_pixels(&ctx, 0).is_none());
    assert_eq!(app.fs_pending.len(), 1);
    assert!(app.final_composite_cache.contains_key(&key));
    tx.send(original_result()).unwrap();
    app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
    assert!(app.ensure_edit_result_pixels(&ctx, 0).is_some());
    assert!(app.fs_pending.is_empty());
}

#[test]
fn original_material_orphan_ai_carries_capture_dimensions_without_current_index_inference() {
    let ctx = egui::Context::default();
    let (mut app, _) = completed_only(&ctx);
    let edit_key = app.current_edit_result_key(0);
    let params = app.effective_params(0).clone();
    let key = app
        .final_ai_key_for_pixels(edit_key, [4, 6], &params)
        .unwrap();
    let retained_key = app.retained_final_ai_key_for(0, key, [4, 6]).unwrap();
    let page_key = RetainedPdfPageKey {
        item_key: retained_key.item_key.clone(),
    };
    let captured = CanonicalPageDimensions::new(1200, 1800);
    app.items[0] = GridItem::PdfPage {
        pdf_path: app.tmp.path().join("replacement.pdf"),
        page_num: 0,
        content_type: Some(crate::pdf_loader::PdfPageContentType::Vector),
    };
    let generation = app.items_generation;
    app.page_dims_cache.record(
        generation,
        0,
        PagePixelDimensions::Canonical(CanonicalPageDimensions::new(900, 700)),
    );
    app.retained_final_ai_orphans.insert(key, 91);
    let retained_epoch = app.retained_final_ai_epoch;
    app.final_ai_result_backlog.push(FinalAiResult::Ready {
        job_id: 91,
        key,
        canonical_dims: Some(captured),
        retained_key: Some(retained_key),
        retained_epoch,
        source_size: [4, 6],
        image: egui::ColorImage::filled([8, 12], egui::Color32::WHITE),
        used_upscale: true,
    });
    app.poll_final_ai(&ctx);
    assert_eq!(
        app.retained_pdf_page_cache[&page_key].canonical_dims,
        Some(captured)
    );
    assert_eq!(app.source_dims_for_idx(0), Some((900.0, 700.0)));
    assert!(app.final_ai_cache.is_empty());
}

fn switch_fixture(ctx: &egui::Context, tool: PageEditTargetSwitchTool) -> AppTestEnvForTest {
    let (mut app, _) = completed_only(ctx);
    app.show_whats_new = false;
    app.show_mouse_nav_migration_prompt = false;
    app.network_data_dir_notice = None;
    app.ai_runtime_init = crate::ai::runtime::AiRuntimeInitOwner::failed_for_test("switch fixture");
    app.fullscreen_idx = Some(21);
    let pivot = PageEditSpreadPivot {
        saved_mode: crate::settings::SpreadMode::Ltr,
        pair: (21, 0),
        navigation_anchor_idx: 21,
    };
    let image = egui::ColorImage::filled([4, 6], egui::Color32::WHITE);
    app.fs_cache.insert(
        21,
        FsCacheEntry::Static {
            tex: ctx.load_texture("switch_old", image.clone(), egui::TextureOptions::LINEAR),
            pixels: Arc::new(image),
            source_dims: Some([4, 6]),
            load_seq: 0,
            animation: StaticAnimationState::Still,
        },
    );
    match tool {
        PageEditTargetSwitchTool::Erase => {
            app.erase_mode = true;
            app.erase_spread_ctx = Some(pivot);
            app.erase_mask_size = [4, 6];
            app.erase_mask = Some(vec![false; 24]);
            app.erase_lasso_points = vec![(0.1, 0.2), (0.3, 0.4)];
        }
        PageEditTargetSwitchTool::LocalAdjustment => {
            app.local_adjust_mode = true;
            app.local_adjust_spread_ctx = Some(pivot);
            app.local_adjust_mask_lasso_points = vec![[0.1, 0.2], [0.3, 0.4]];
        }
        PageEditTargetSwitchTool::Conceal => {
            app.conceal_mode = true;
            app.conceal_spread_ctx = Some(pivot);
            app.conceal_mask_size = [4, 6];
            let mut mask = vec![false; 24];
            mask[3] = true;
            app.conceal_mask = Some(mask);
        }
    }
    app
}

fn request_switch(app: &mut App, ctx: &egui::Context, tool: PageEditTargetSwitchTool) {
    match tool {
        PageEditTargetSwitchTool::Erase => app.switch_erase_target_in_spread(ctx, 0),
        PageEditTargetSwitchTool::LocalAdjustment => {
            app.switch_local_adjust_target_in_spread(ctx, 0)
        }
        PageEditTargetSwitchTool::Conceal => app.switch_conceal_target_in_spread(ctx, 0),
    }
}

fn finish_switch_materializers(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app.erase_inpaint_pending.is_empty() || !app.local_adjust_pending.is_empty() {
        app.frame_counter += 1;
        app.poll_erase_inpaint(ctx);
        app.poll_local_adjust_render(ctx);
        assert!(
            Instant::now() < deadline,
            "switch must finish through existing workers"
        );
        std::thread::yield_now();
    }
}

#[test]
fn original_material_target_switch_keeps_old_editor_until_target_ready_then_commits_once() {
    for tool in [
        PageEditTargetSwitchTool::Erase,
        PageEditTargetSwitchTool::LocalAdjustment,
        PageEditTargetSwitchTool::Conceal,
    ] {
        let ctx = egui::Context::default();
        let mut app = switch_fixture(&ctx, tool);
        let before = app.page_edit_target_switch_request(tool, 0).unwrap();
        let old_conceal_mask = app.conceal_mask.clone();
        let old_erase_mask = app.erase_mask.clone();
        let old_lasso = app.local_adjust_mask_lasso_points.clone();
        let tx = inject_pending(&mut app, OriginalMaterialEntry::MaterialOnly);
        request_switch(&mut app, &ctx, tool);
        assert_eq!(
            app.fs_pending.get(&0).unwrap().purpose.original_entry(),
            Some(OriginalMaterialEntry::Switch { request: before })
        );
        app.reconcile_page_edit_spread_pivots();
        assert_eq!(app.fullscreen_idx, Some(21));
        assert_eq!(app.page_edit_target_switch_request(tool, 0), Some(before));
        assert_eq!(app.conceal_mask, old_conceal_mask);
        assert_eq!(app.erase_mask, old_erase_mask);
        assert_eq!(app.local_adjust_mask_lasso_points, old_lasso);
        tx.send(original_result()).unwrap();
        app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
        if tool == PageEditTargetSwitchTool::Conceal {
            assert_eq!(
                app.fullscreen_idx,
                Some(21),
                "saved target mask is not yet received"
            );
            assert_eq!(app.conceal_mask, old_conceal_mask);
            app.reconcile_page_edit_spread_pivots();
            assert_eq!(app.fullscreen_idx, Some(21));
        }
        finish_switch_materializers(&mut app, &ctx);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.spread_mode, crate::settings::SpreadMode::Single);
        match tool {
            PageEditTargetSwitchTool::Erase => {
                assert!(app.erase_mode);
                assert!(app.erase_lasso_points.is_empty());
            }
            PageEditTargetSwitchTool::LocalAdjustment => {
                assert!(app.local_adjust_mode);
                assert!(app.local_adjust_mask_lasso_points.is_empty());
            }
            PageEditTargetSwitchTool::Conceal => {
                assert!(app.conceal_mode);
                assert!(!app.conceal_mask.as_ref().unwrap().iter().any(|bit| *bit));
            }
        }
        assert!(app.fs_pending.is_empty());
        assert!(app.page_edit_target_switch_request(tool, 21).is_some());
    }
}

#[test]
fn original_material_target_switch_cancel_failure_and_mode_reentry_leave_old_editor() {
    for tool in [
        PageEditTargetSwitchTool::Erase,
        PageEditTargetSwitchTool::LocalAdjustment,
        PageEditTargetSwitchTool::Conceal,
    ] {
        for failure in [false, true] {
            let ctx = egui::Context::default();
            let mut app = switch_fixture(&ctx, tool);
            let tx = inject_pending(&mut app, OriginalMaterialEntry::MaterialOnly);
            request_switch(&mut app, &ctx, tool);
            if failure {
                tx.send(FsLoadResult::Failed).unwrap();
            } else {
                // OFF -> same old-page re-entry must not revive the retired operation.
                app.cancel_original_material_entry_intents(None);
                match tool {
                    PageEditTargetSwitchTool::Erase => {
                        app.erase_mode = false;
                        app.erase_mode = true;
                    }
                    PageEditTargetSwitchTool::LocalAdjustment => {
                        app.local_adjust_mode = false;
                        app.local_adjust_mode = true;
                    }
                    PageEditTargetSwitchTool::Conceal => {
                        app.conceal_mode = false;
                        app.conceal_mode = true;
                    }
                }
                tx.send(original_result()).unwrap();
            }
            app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
            finish_switch_materializers(&mut app, &ctx);
            assert_eq!(app.fullscreen_idx, Some(21));
            assert!(app.page_edit_target_switch_request(tool, 0).is_some());
        }
    }
}

#[test]
fn original_material_conceal_switch_cancels_cold_mask_owner_without_reviving_after_reentry() {
    let ctx = egui::Context::default();
    let mut app = switch_fixture(&ctx, PageEditTargetSwitchTool::Conceal);
    let tx = inject_pending(&mut app, OriginalMaterialEntry::MaterialOnly);
    tx.send(original_result()).unwrap();
    app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
    request_switch(&mut app, &ctx, PageEditTargetSwitchTool::Conceal);
    let cancellation = Arc::clone(&app.local_adjust_pending[&0].cancel);
    app.cancel_original_material_entry_intents(None);
    assert!(cancellation.load(Ordering::Relaxed));
    assert!(app.local_adjust_pending.is_empty());
    app.conceal_mode = false;
    app.conceal_mode = true;
    app.frame_counter += 1;
    app.poll_local_adjust_render(&ctx);
    assert_eq!(app.fullscreen_idx, Some(21));
    assert!(app.conceal_mask.as_ref().unwrap()[3]);
}

#[test]
fn original_material_switch_joins_ready_display_worker_with_immutable_input_identity() {
    for local in [false, true] {
        for cancelled in [false, true] {
            let ctx = egui::Context::default();
            let mut app = switch_fixture(&ctx, PageEditTargetSwitchTool::Conceal);
            let tx = inject_pending(&mut app, OriginalMaterialEntry::MaterialOnly);
            tx.send(original_result()).unwrap();
            app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
            let source = app.current_raw_source_pixels(0).unwrap();
            if local {
                app.local_adjust_pages.insert(0);
            }
            let request = EditMaterializeRequest {
                items_generation: app.items_generation,
                input_seq: app.input_seq,
                kind: if local {
                    EditMaterializeKind::Local {
                        key: app.current_local_adjust_key(0),
                        continuation: LocalMaterializeContinuation::Display,
                    }
                } else {
                    EditMaterializeKind::Conceal {
                        key: app.current_edit_result_key(0),
                        purpose: ConcealMaterializePurpose::Display,
                    }
                },
            };
            let payload = if local {
                EditMaterializePayload::LocalInactive {
                    layers: Arc::new(Vec::new()),
                }
            } else {
                EditMaterializePayload::ConcealDisplayEmpty {
                    source,
                    source_kind: "fs",
                }
            };
            let (_tx, rx) = mpsc::channel();
            let cancellation = Arc::new(AtomicBool::new(false));
            app.local_adjust_pending.insert(
                0,
                LocalAdjustRenderPending {
                    request,
                    cancel: Arc::clone(&cancellation),
                    rx,
                    ready: Some(EditMaterializeResult::Ready { request, payload }),
                },
            );
            app.input_seq = app.input_seq.wrapping_add(7);
            request_switch(&mut app, &ctx, PageEditTargetSwitchTool::Conceal);
            let owner = &app.local_adjust_pending[&0];
            assert_eq!(owner.request.input_seq, request.input_seq);
            assert!(request.same_worker_input(owner.request));
            assert!(Arc::ptr_eq(&owner.cancel, &cancellation));
            assert!(matches!(
                owner.request.kind,
                EditMaterializeKind::Local {
                    continuation: LocalMaterializeContinuation::ResumeAfterDisplaySwitchConceal { .. },
                    ..
                } | EditMaterializeKind::Conceal {
                    purpose: ConcealMaterializePurpose::ResumeAfterDisplaySwitch { .. },
                    ..
                }
            ));
            if cancelled {
                app.cancel_original_material_entry_intents(None);
                assert_eq!(app.local_adjust_pending[&0].request, request);
                assert!(!cancellation.load(Ordering::Relaxed));
            }
            app.frame_counter += 1;
            app.poll_local_adjust_render(&ctx);
            finish_switch_materializers(&mut app, &ctx);
            assert_eq!(app.fullscreen_idx, Some(if cancelled { 21 } else { 0 }));
            assert!(app.conceal_mode);
        }
    }
}

#[test]
fn original_material_target_switch_waits_for_saved_erase_and_conceal_mask_without_losing_old_editor()
 {
    for tool in [
        PageEditTargetSwitchTool::LocalAdjustment,
        PageEditTargetSwitchTool::Conceal,
    ] {
        let ctx = egui::Context::default();
        let mut app = switch_fixture(&ctx, tool);
        let page_key = app.page_path_key(0).unwrap();
        let mut saved_mask = vec![false; 24];
        saved_mask[9] = true;
        if app.mask_db.is_none() {
            app.mask_db = Some(crate::mask_db::MaskDb::open().unwrap());
        }
        app.mask_db
            .as_ref()
            .unwrap()
            .set(&page_key, &saved_mask, &[], 4, 6)
            .unwrap();
        app.mask_pages.insert(0);
        app.mask_page_keys.insert(page_key.clone());
        if tool == PageEditTargetSwitchTool::Conceal {
            if app.conceal_db.is_none() {
                app.conceal_db = Some(crate::conceal_db::ConcealDb::open().unwrap());
            }
            app.conceal_db
                .as_ref()
                .unwrap()
                .set(&page_key, &saved_mask, &[], 4, 6)
                .unwrap();
            app.conceal_pages.insert(0);
            app.conceal_page_keys.insert(page_key);
        }
        let final_key = *app
            .final_composite_cache
            .keys()
            .find(|key| key.edit_key.idx == 0)
            .unwrap();
        let final_pixels = Arc::clone(&app.final_composite_cache[&final_key].pixels);
        let source_tx = inject_pending(&mut app, OriginalMaterialEntry::MaterialOnly);
        request_switch(&mut app, &ctx, tool);
        source_tx.send(original_result()).unwrap();
        app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
        assert_eq!(app.fullscreen_idx, Some(21));
        assert!(!app.erase_inpaint_pending.is_empty());
        app.reconcile_page_edit_spread_pivots();
        assert_eq!(app.fullscreen_idx, Some(21));
        app.fs_cache.remove(&0);
        finish_switch_materializers(&mut app, &ctx);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(app.current_erase_result_pixels(0).is_some());
        assert!(app.current_raw_source_pixels(0).is_none());
        if tool == PageEditTargetSwitchTool::Conceal {
            assert_eq!(app.conceal_mask, Some(saved_mask));
        } else {
            assert!(app.local_adjust_mode);
        }
        assert!(Arc::ptr_eq(
            &app.final_composite_cache[&final_key].pixels,
            &final_pixels
        ));
    }
}
