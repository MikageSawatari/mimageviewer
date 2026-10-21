#![cfg(test)]

use super::*;
use crate::app::composite_pixels_revision_tests::{evict_pair_cpu, warm_final_pair};
use crate::page_dims::{CanonicalPageDimensions, PagePixelDimensions};

const CANONICAL: [usize; 2] = [1200, 1800];

#[derive(Debug, Clone, Copy)]
enum SupersedingConcealEntry {
    Text,
    Crop,
    Analysis,
    Export,
}

fn select_superseding_conceal_entry(
    app: &mut App,
    ctx: &egui::Context,
    entry: SupersedingConcealEntry,
) {
    match entry {
        SupersedingConcealEntry::Text => app.enter_text_mode(0),
        SupersedingConcealEntry::Crop => app.enter_export_crop_mode(0),
        SupersedingConcealEntry::Analysis => app.toggle_analysis_mode(),
        SupersedingConcealEntry::Export => app.open_export_dialog_for_current(ctx, 0),
    }
}

fn assert_superseding_conceal_entry(app: &App, entry: SupersedingConcealEntry) {
    assert!(
        !app.conceal_mode,
        "late initial Conceal entry survived {entry:?}"
    );
    match entry {
        SupersedingConcealEntry::Text => assert!(app.text_mode),
        SupersedingConcealEntry::Crop => assert!(app.export_crop_mode),
        SupersedingConcealEntry::Analysis => assert!(app.analysis_mode),
        SupersedingConcealEntry::Export => assert!(app.export_dialog.is_some()),
    }
}

#[test]
fn retained_material_entry_initial_conceal_materializers_cancel_on_another_real_entry() {
    use crate::app::{
        ConcealMaterializePurpose, EditMaterializeKind, EditMaterializePayload,
        EditMaterializeRequest, EditMaterializeResult, LocalAdjustRenderPending,
        LocalMaterializeContinuation,
    };

    let ctx = egui::Context::default();
    for local_stage in [false, true] {
        for ready in [false, true] {
            for entry in [
                SupersedingConcealEntry::Text,
                SupersedingConcealEntry::Crop,
                SupersedingConcealEntry::Analysis,
                SupersedingConcealEntry::Export,
            ] {
                let mut app = final_only_page(&ctx);
                let final_texture = app.current_final_composite_texture(0).unwrap().id();
                let request = EditMaterializeRequest {
                    items_generation: app.items_generation,
                    input_seq: app.input_seq,
                    kind: if local_stage {
                        EditMaterializeKind::Local {
                            key: app.current_local_adjust_key(0),
                            continuation: LocalMaterializeContinuation::EnterConceal {
                                requested_fs_idx: 0,
                            },
                        }
                    } else {
                        EditMaterializeKind::Conceal {
                            key: app.current_edit_result_key(0),
                            purpose: ConcealMaterializePurpose::EnterMode {
                                requested_fs_idx: 0,
                            },
                        }
                    },
                };
                let source = Arc::new(egui::ColorImage::filled([4, 6], egui::Color32::GREEN));
                let payload = if local_stage {
                    EditMaterializePayload::LocalReady {
                        layers: Arc::new(vec![local_adjust_core::LocalAdjustmentLayer::new(
                            "completed-for-conceal",
                            local_adjust_core::LocalMask::Full,
                            local_adjust_core::LocalEffect::None,
                        )]),
                        image: (*source).clone(),
                    }
                } else {
                    EditMaterializePayload::ConcealEnter {
                        source,
                        source_kind: "local_adjust",
                        mask: vec![false; 24],
                        shapes: Vec::new(),
                    }
                };
                let result = EditMaterializeResult::Ready { request, payload };
                let (sender, rx) = std::sync::mpsc::channel();
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                app.local_adjust_pending.insert(
                    0,
                    LocalAdjustRenderPending {
                        request,
                        cancel: Arc::clone(&cancel),
                        rx,
                        ready: None,
                    },
                );
                let late_result = if ready {
                    app.local_adjust_pending.get_mut(&0).unwrap().ready = Some(result);
                    None
                } else {
                    Some(result)
                };

                select_superseding_conceal_entry(&mut app, &ctx, entry);

                assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
                assert!(
                    app.local_adjust_pending.is_empty(),
                    "{local_stage}/{ready}/{entry:?}"
                );
                if let Some(result) = late_result {
                    assert!(
                        sender.send(result).is_err(),
                        "cancelled entry still owns its receiver"
                    );
                }
                app.frame_counter += 1;
                app.poll_local_adjust_render(&ctx);
                assert_superseding_conceal_entry(&app, entry);
                assert_eq!(
                    app.current_final_composite_texture(0).unwrap().id(),
                    final_texture
                );
                match entry {
                    SupersedingConcealEntry::Analysis => {
                        assert_eq!(app.fs_pending.len(), 1);
                        assert_eq!(
                            app.fs_pending.get(&0).unwrap().purpose,
                            crate::app::FsLoadPurpose::Original {
                                entry: crate::app::OriginalMaterialEntry::MaterialOnly,
                            }
                        );
                    }
                    _ => assert!(
                        app.fs_pending.is_empty(),
                        "unexpected source request: {local_stage}/{ready}/{entry:?}"
                    ),
                }
                assert!(app.fs_upload_backlog.is_empty());
            }
        }
    }
}

#[test]
fn retained_material_entry_initial_conceal_materializers_keep_requested_anchor_only() {
    use crate::app::{
        ConcealMaterializePurpose, EditMaterializeKind, EditMaterializeRequest,
        LocalAdjustRenderPending, LocalMaterializeContinuation,
    };
    let ctx = egui::Context::default();
    for local_stage in [false, true] {
        let mut app = final_only_page(&ctx);
        let request = EditMaterializeRequest {
            items_generation: app.items_generation,
            input_seq: app.input_seq,
            kind: if local_stage {
                EditMaterializeKind::Local {
                    key: app.current_local_adjust_key(0),
                    continuation: LocalMaterializeContinuation::EnterConceal {
                        requested_fs_idx: 21,
                    },
                }
            } else {
                EditMaterializeKind::Conceal {
                    key: app.current_edit_result_key(0),
                    purpose: ConcealMaterializePurpose::EnterMode {
                        requested_fs_idx: 21,
                    },
                }
            },
        };
        let (_sender, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        app.local_adjust_pending.insert(
            0,
            LocalAdjustRenderPending {
                request,
                cancel: Arc::clone(&cancel),
                rx,
                ready: None,
            },
        );
        app.cancel_original_material_entry_intents(Some(21));
        assert_eq!(app.local_adjust_pending.get(&0).unwrap().request, request);
        assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));
        app.cancel_original_material_entry_intents(Some(0));
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert!(app.local_adjust_pending.is_empty());
    }
}

/// Actual Retained restoration followed by CPU eviction, with three different
/// pixel bases. Thumbnail pixels must never become annotation coordinates.
fn final_only_page(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    let (mut app, keys) = warm_final_pair(ctx);
    let generation = app.items_generation;
    app.page_dims_cache.record(
        generation,
        0,
        PagePixelDimensions::Canonical(CanonicalPageDimensions::new(1200, 1800)),
    );
    app.final_composite_cache
        .get_mut(&keys[1])
        .unwrap()
        .canonical_dims = Some(CanonicalPageDimensions::new(1200, 1800));
    let item_count = app.items.len();
    app.thumbnails
        .resize_with(item_count, || ThumbnailState::Pending);
    app.thumbnails[0] = ThumbnailState::Loaded {
        tex: ctx.load_texture(
            "retained-entry-thumbnail",
            egui::ColorImage::filled([160, 240], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        ),
        origin: crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
        from_edit_preview: false,
        rendered_at_px: 240,
        source_coverage: crate::source_coverage::SourceCoverage::FullSource,
        source_dims: Some((160, 240)),
        layout_dims: Some((600_000, 900_000)),
    };
    app.page_dims_cache
        .record(generation, 0, PagePixelDimensions::Thumbnail((160, 240)));
    evict_pair_cpu(&mut app);
    app.fullscreen_idx = Some(0);
    app.spread_mode = SpreadMode::Single;
    app.viewer_navigation_caches.invalidate();
    app.rotation_cache
        .insert(0, crate::rotation_db::Rotation::None);
    // Avoid font discovery and optional add-on prompts in entry-only tests.
    app.comic_font_registry_loaded = true;
    app.comic_fonts_loaded = true;
    app.comic_fonts = Some(Arc::new(comic_core::FontSet::new()));
    app.editing_addon_declined_session = true;
    app.show_whats_new = false;
    app.show_mouse_nav_migration_prompt = false;
    app.network_data_dir_notice = None;
    assert!(
        !app.any_modal_dialog_open_for_fullscreen_keys(),
        "entry fixture must begin with no modal"
    );
    assert_final_only(&app);
    app
}

fn assert_final_only(app: &App) {
    assert!(!app.fs_cache.contains_key(&0));
    assert!(!app.fs_pending.contains_key(&0));
    assert!(!app.fs_upload_backlog_contains(0));
    assert!(app.edit_result_cache.is_empty());
    assert_eq!(app.source_dims_for_idx(0), Some((1200.0, 1800.0)));
    assert_eq!(
        app.pre_ai_display_size_for_idx(0),
        Some(egui::vec2(4.0, 6.0))
    );
    assert_eq!(
        app.current_final_composite_texture(0).unwrap().size(),
        [8, 12]
    );
}

#[test]
fn retained_material_entry_ctrl_e_prepares_dialog_without_original_reload() {
    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    app.open_export_dialog_for_current(&ctx, 0);
    let dialog = app
        .export_dialog
        .as_ref()
        .expect("Ctrl+E must reach its dialog");
    let crate::export_dialog::ExportComposite::Single(page) = &dialog.composite else {
        panic!("single-page entry must keep its export scope");
    };
    assert_eq!(page.edits.comic_source_dims, Some(CANONICAL));
    assert!(page.predicted_size.iter().all(|&dimension| dimension > 0));
    assert_final_only(&app);
}

#[test]
fn retained_material_entry_final_metadata_supplies_coordinates_when_cache_has_only_thumbnail() {
    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    app.page_dims_cache.clear();
    let generation = app.items_generation;
    app.page_dims_cache
        .record(generation, 0, PagePixelDimensions::Thumbnail((160, 240)));
    assert!(
        app.page_dims_cache
            .get_canonical(app.items_generation, 0)
            .is_none()
    );
    assert_final_only(&app);
    assert_eq!(
        app.cached_top_bar_page_info(0).image_dims,
        Some((1200, 1800))
    );
    app.open_export_dialog_for_current(&ctx, 0);
    let crate::export_dialog::ExportComposite::Single(page) = &app
        .export_dialog
        .as_ref()
        .expect("final metadata must admit Ctrl+E")
        .composite
    else {
        panic!("single-page export");
    };
    assert_eq!(page.edits.comic_source_dims, Some(CANONICAL));
    assert_final_only(&app);
}

#[test]
fn retained_material_entry_dimension_tools_enter_after_cpu_eviction() {
    let ctx = egui::Context::default();
    let mut crop = final_only_page(&ctx);
    crop.enter_export_crop_mode(0);
    assert!(crop.export_crop_mode);
    assert_final_only(&crop);
    drop(crop);

    let mut text = final_only_page(&ctx);
    text.enter_text_mode(0);
    assert!(text.text_mode);
    assert_eq!(text.fullscreen_idx, Some(0));
    assert_final_only(&text);
    drop(text);

    let mut sns = final_only_page(&ctx);
    sns.enter_sns_split_mode(0)
        .expect("SNS entry needs dimensions, not raw pixels");
    assert!(sns.sns_split.is_some());
    assert_eq!(
        sns.fs_page_coordinate_source_size(0),
        Some(egui::vec2(1200.0, 1800.0))
    );
    assert_final_only(&sns);
}

#[test]
fn retained_material_entry_capture_and_region_selection_use_completed_output() {
    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    let work = app
        .prepare_capture_pixel_work(&ctx, 0)
        .expect("capture entry must be ready");
    let crate::capture::CapturePixelWork::Single(job) = work else {
        panic!("single-page capture");
    };
    assert_eq!(job.source.size, [8, 12]);
    app.begin_capture_region_selection(&ctx, 0);
    assert!(app.capture_region_selection.is_some());
    assert_final_only(&app);
}

#[test]
fn retained_material_entry_comparison_and_rotation_keep_completed_pixels() {
    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    let pinned = Arc::new(egui::ColorImage::filled([8, 12], egui::Color32::BLUE));
    app.pinned_compare_slot = Some(crate::app::PinnedCompareSlot {
        pixels: Arc::clone(&pinned),
        indicator_pixels: pinned,
        indicator_texture: None,
        display_name: "retained-entry-pin".into(),
        source_idx: 21,
        external_item_key: None,
        source_size: [8, 12],
    });
    app.toggle_compare_wipe_mode(&ctx, 0);
    assert!(matches!(
        app.compare_view_mode,
        crate::app::CompareViewMode::Wipe { .. }
    ));
    assert!(matches!(
        app.prepare_compare_current_job(&ctx, 0).unwrap(),
        CompareCurrentJobPreparation::Ready { .. }
    ));
    assert_final_only(&app);
    app.toggle_compare_wipe_mode(&ctx, 0);
    app.rotate_image_cw(0);
    assert_eq!(app.get_rotation(0), crate::rotation_db::Rotation::Cw90);
    assert_final_only(&app);
}

fn pending_original(
    app: &mut App,
) -> (
    std::sync::mpsc::Sender<crate::fs_animation::FsLoadResult>,
    Arc<std::sync::atomic::AtomicBool>,
) {
    pending_original_at(app, 0)
}

fn pending_original_at(
    app: &mut App,
    idx: usize,
) -> (
    std::sync::mpsc::Sender<crate::fs_animation::FsLoadResult>,
    Arc<std::sync::atomic::AtomicBool>,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    app.fs_pending.insert(
        idx,
        crate::app::FsPendingValue::new(
            Arc::clone(&cancel),
            rx,
            123,
            crate::app::FsLoadPurpose::Original {
                entry: crate::app::OriginalMaterialEntry::MaterialOnly,
            },
        ),
    );
    (tx, cancel)
}

fn assert_original_continuation(
    app: &App,
    cancel: &Arc<std::sync::atomic::AtomicBool>,
    expected: crate::app::OriginalMaterialEntry,
) {
    let pending = app
        .fs_pending
        .get(&0)
        .expect("entry must retain the existing load");
    assert!(Arc::ptr_eq(&pending.cancel, cancel));
    assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(pending.load_seq, 123);
    assert_eq!(
        pending.purpose,
        crate::app::FsLoadPurpose::Original { entry: expected }
    );
    assert!(!app.fs_cache.contains_key(&0));
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.spread_mode, SpreadMode::Single);
}

#[test]
fn retained_material_entry_original_edit_tools_wait_on_the_existing_load_owner() {
    use crate::app::OriginalMaterialEntry;

    let ctx = egui::Context::default();
    let mut erase = final_only_page(&ctx);
    let (_sender, cancel) = pending_original(&mut erase);
    erase.enter_erase_mode(0);
    assert!(
        !erase.erase_mode,
        "final output cannot become the erase base"
    );
    assert!(erase.erase_base_cache.is_empty());
    assert_original_continuation(
        &erase,
        &cancel,
        OriginalMaterialEntry::Erase {
            requested_fs_idx: 0,
        },
    );

    drop(erase);

    let mut conceal = final_only_page(&ctx);
    let (_sender, cancel) = pending_original(&mut conceal);
    conceal.enter_conceal_mode(&ctx, 0);
    assert!(
        !conceal.conceal_mode,
        "final output cannot become the conceal base"
    );
    assert!(conceal.conceal_base_cache.is_empty());
    assert_original_continuation(
        &conceal,
        &cancel,
        OriginalMaterialEntry::Conceal {
            requested_fs_idx: 0,
        },
    );

    drop(conceal);

    let mut local = final_only_page(&ctx);
    let (_sender, cancel) = pending_original(&mut local);
    local.enter_local_adjust_mode(&ctx);
    assert!(!local.local_adjust_mode);
    assert_original_continuation(
        &local,
        &cancel,
        OriginalMaterialEntry::LocalAdjustment {
            requested_fs_idx: 0,
        },
    );

    drop(local);

    let mut crop = final_only_page(&ctx);
    let (_sender, cancel) = pending_original(&mut crop);
    assert!(!crop.apply_auto_export_crop_for_idx(0));
    assert!(!crop.export_crop_page_settings.contains_key(&0));
    assert_original_continuation(&crop, &cancel, OriginalMaterialEntry::AutoCrop);
}

#[test]
fn retained_material_entry_analysis_requests_original_pixels_without_using_final_output() {
    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    let (_sender, cancel) = pending_original(&mut app);
    app.toggle_analysis_mode();
    assert!(app.analysis_mode);
    assert!(app.resolve_fs_processed_texture(&ctx, 0, false).is_none());
    assert_original_continuation(
        &app,
        &cancel,
        crate::app::OriginalMaterialEntry::MaterialOnly,
    );
}

fn seed_current_erase_stage(app: &mut App, ctx: &egui::Context) -> Arc<egui::ColorImage> {
    let pixels = Arc::new(egui::ColorImage::filled(
        [4, 6],
        egui::Color32::from_rgb(41, 71, 101),
    ));
    let key = app.current_erase_result_key(0);
    let texture = ctx.load_texture(
        "retained-entry-current-erase-stage",
        (*pixels).clone(),
        egui::TextureOptions::LINEAR,
    );
    app.erase_result_cache.insert(
        key,
        crate::app::EraseResultCacheEntry {
            pixels: Arc::clone(&pixels),
            texture,
        },
    );
    app.mask_pages.insert(0);
    pixels
}

#[test]
fn retained_material_entry_local_adjustment_uses_current_edit_stage_without_original_reload() {
    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    let pixels = seed_current_erase_stage(&mut app, &ctx);
    let generation = app.input_generation.get(&0).copied();

    app.enter_local_adjust_mode(&ctx);

    assert!(app.local_adjust_mode);
    assert_eq!(
        app.current_local_adjust_source_pixels(0).unwrap().pixels,
        pixels.pixels
    );
    assert_eq!(app.input_generation.get(&0).copied(), generation);
    assert!(!app.fs_cache.contains_key(&0));
    assert!(!app.fs_pending.contains_key(&0));
    assert!(!app.fs_upload_backlog_contains(0));
}

#[test]
fn retained_material_entry_conceal_uses_current_edit_stage_without_original_reload() {
    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    let pixels = seed_current_erase_stage(&mut app, &ctx);
    let generation = app.input_generation.get(&0).copied();

    app.enter_conceal_mode(&ctx, 0);
    for _ in 0..200 {
        app.poll_local_adjust_render(&ctx);
        if app.conceal_mode {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    assert!(app.conceal_mode);
    assert_eq!(
        app.conceal_base_cache.get(&0).unwrap().pixels,
        pixels.pixels
    );
    assert_eq!(app.input_generation.get(&0).copied(), generation);
    assert!(!app.fs_cache.contains_key(&0));
    assert!(!app.fs_pending.contains_key(&0));
    assert!(!app.fs_upload_backlog_contains(0));
}

#[test]
fn retained_material_entry_original_completion_resumes_the_requested_erase_entry() {
    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    let (sender, cancel) = pending_original(&mut app);
    app.enter_erase_mode(0);
    assert!(!app.erase_mode);
    assert_original_continuation(
        &app,
        &cancel,
        crate::app::OriginalMaterialEntry::Erase {
            requested_fs_idx: 0,
        },
    );
    // Ordinary original-preview readiness is a consumer of the same worker;
    // asking for its pixels must not supersede the operation awaiting adoption.
    assert!(app.resolve_fs_processed_texture(&ctx, 0, true).is_none());
    assert_original_continuation(
        &app,
        &cancel,
        crate::app::OriginalMaterialEntry::Erase {
            requested_fs_idx: 0,
        },
    );
    sender
        .send(crate::fs_animation::FsLoadResult::Static {
            ci: egui::ColorImage::filled([4, 6], egui::Color32::GREEN),
            source_dims: CANONICAL,
            animation: crate::fs_animation::StaticAnimationState::Still,
        })
        .unwrap();
    app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
    assert!(
        app.erase_mode,
        "original adoption must resume the accepted operation"
    );
    let base = app.erase_base_cache.get(&0).unwrap();
    assert_eq!(base.size, [4, 6]);
    assert!(
        base.pixels
            .iter()
            .all(|&pixel| pixel == egui::Color32::GREEN)
    );
    assert_eq!(app.erase_mask_size, [4, 6]);
    assert!(!app.fs_pending.contains_key(&0));
}

#[test]
fn retained_material_entry_original_completion_resumes_local_adjustment_and_auto_crop() {
    let ctx = egui::Context::default();
    let mut local = final_only_page(&ctx);
    let (sender, _) = pending_original(&mut local);
    local.enter_local_adjust_mode(&ctx);
    assert!(!local.local_adjust_mode);
    sender
        .send(crate::fs_animation::FsLoadResult::Static {
            ci: egui::ColorImage::filled([4, 6], egui::Color32::GREEN),
            source_dims: CANONICAL,
            animation: crate::fs_animation::StaticAnimationState::Still,
        })
        .unwrap();
    local.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
    assert!(local.local_adjust_mode);
    assert!(local.current_raw_source_pixels(0).is_some());
    drop(local);

    let mut crop = final_only_page(&ctx);
    let (sender, _) = pending_original(&mut crop);
    assert!(!crop.apply_auto_export_crop_for_idx(0));
    let mut original = egui::ColorImage::filled([100, 80], egui::Color32::WHITE);
    for y in 16..64 {
        for x in 20..80 {
            original[(x, y)] = egui::Color32::BLACK;
        }
    }
    sender
        .send(crate::fs_animation::FsLoadResult::Static {
            ci: original,
            source_dims: [100, 80],
            animation: crate::fs_animation::StaticAnimationState::Still,
        })
        .unwrap();
    crop.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
    let settings = crop
        .export_crop_page_settings
        .get(&0)
        .expect("original adoption must perform the deferred auto crop");
    assert_eq!(settings.valid_source_size(), Some([100, 80]));
    assert!(settings.rect.min_x > 0.0 && settings.rect.min_y > 0.0);
    assert!(settings.rect.max_x < 100.0 && settings.rect.max_y < 80.0);
}

#[test]
fn retained_material_entry_original_preview_selects_only_the_requested_original() {
    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    let (sender, cancel) = pending_original(&mut app);
    assert!(app.resolve_fs_processed_texture(&ctx, 0, true).is_none());
    assert_original_continuation(
        &app,
        &cancel,
        crate::app::OriginalMaterialEntry::MaterialOnly,
    );
    sender
        .send(crate::fs_animation::FsLoadResult::Static {
            ci: egui::ColorImage::filled([4, 6], egui::Color32::GREEN),
            source_dims: CANONICAL,
            animation: crate::fs_animation::StaticAnimationState::Still,
        })
        .unwrap();
    app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
    let original = app
        .resolve_fs_processed_texture(&ctx, 0, true)
        .expect("original preview must become ready on source adoption");
    assert_eq!(
        original.size(),
        [4, 6],
        "preview must not sample the enlarged final output"
    );
    let FsCacheEntry::Static { tex, .. } = app.fs_cache.get(&0).unwrap() else {
        panic!("adopted original must belong to this page's source cache");
    };
    assert_eq!(original.id(), tex.id());
    assert!(!app.erase_mode && !app.conceal_mode && !app.local_adjust_mode);
}

#[test]
fn retained_material_entry_saved_manual_trim_uses_final_while_auto_trim_requests_original() {
    use crate::view_trim::{ViewTrimApplyMode, ViewTrimMargins, ViewTrimPageOverride};

    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    app.view_trim_apply_mode = ViewTrimApplyMode::Book;
    app.view_trim_page_overrides.insert(
        0,
        ViewTrimPageOverride::from_margins(ViewTrimMargins {
            left: 0.10,
            top: 0.10,
            right: 0.20,
            bottom: 0.20,
        }),
    );
    assert_eq!(
        app.view_trim_single_content_bbox(0),
        Some(egui::Rect::from_min_max(
            egui::pos2(0.10, 0.10),
            egui::pos2(0.80, 0.80),
        ))
    );
    assert_final_only(&app);
    app.view_trim_page_overrides.clear();
    app.view_trim_apply_mode = ViewTrimApplyMode::Auto;
    let (sender, cancel) = pending_original(&mut app);
    assert!(app.view_trim_single_content_bbox(0).is_none());
    assert_original_continuation(
        &app,
        &cancel,
        crate::app::OriginalMaterialEntry::MaterialOnly,
    );
    let mut original = egui::ColorImage::filled([100, 80], egui::Color32::WHITE);
    for y in 16..64 {
        for x in 20..80 {
            original[(x, y)] = egui::Color32::BLACK;
        }
    }
    sender
        .send(crate::fs_animation::FsLoadResult::Static {
            ci: original,
            source_dims: [100, 80],
            animation: crate::fs_animation::StaticAnimationState::Still,
        })
        .unwrap();
    app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
    let bbox = app
        .view_trim_single_content_bbox(0)
        .expect("automatic margin detection must use the adopted original");
    assert!(bbox.min.x > 0.0 && bbox.min.y > 0.0);
    assert!(bbox.max.x < 1.0 && bbox.max.y < 1.0);
}

#[test]
fn retained_material_entry_viewing_erase_slots_resume_after_original_adoption() {
    use crate::app::OriginalMaterialEntry;

    let ctx = egui::Context::default();
    for slot in [1, 2] {
        let mut app = final_only_page(&ctx);
        let (sender, cancel) = pending_original(&mut app);
        app.apply_slot_in_viewing_mode(&ctx, slot);
        assert_original_continuation(
            &app,
            &cancel,
            OriginalMaterialEntry::ApplyEraseSlot {
                requested_fs_idx: 0,
                slot,
            },
        );
        assert!(!app.erase_mode);
        assert!(app.erase_inpaint_pending.is_empty());
        // These disposable slots are deliberately empty: adoption must resume
        // the real slot handler and its existing empty-slot result, without AI.
        sender
            .send(crate::fs_animation::FsLoadResult::Static {
                ci: egui::ColorImage::filled([4, 6], egui::Color32::GREEN),
                source_dims: CANONICAL,
                animation: crate::fs_animation::StaticAnimationState::Still,
            })
            .unwrap();
        app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
        let toast = &app
            .fs_feedback_toast
            .as_ref()
            .expect("slot completion must give its result")
            .0;
        assert_eq!(toast, &format!("[消しゴムスロット{slot}は空です]"));
        assert!(!app.erase_mode);
        assert!(app.erase_inpaint_pending.is_empty());
        assert!(!app.mask_pages.contains(&0));
        assert!(!app.fs_pending.contains_key(&0));
    }
}

#[test]
fn retained_material_entry_new_mode_or_export_dialog_prevents_deferred_erase_activation() {
    #[derive(Debug, Clone, Copy)]
    enum NewEntry {
        Analysis,
        Text,
        Crop,
        ExportDialog,
    }

    let ctx = egui::Context::default();
    for entry in [
        NewEntry::Analysis,
        NewEntry::Text,
        NewEntry::Crop,
        NewEntry::ExportDialog,
    ] {
        let mut app = final_only_page(&ctx);
        let (sender, cancel) = pending_original(&mut app);
        app.enter_erase_mode(0);
        assert_original_continuation(
            &app,
            &cancel,
            crate::app::OriginalMaterialEntry::Erase {
                requested_fs_idx: 0,
            },
        );
        match entry {
            NewEntry::Analysis => {
                app.toggle_analysis_mode();
                assert!(app.analysis_mode);
            }
            NewEntry::Text => {
                app.enter_text_mode(0);
                assert!(app.text_mode);
            }
            NewEntry::Crop => {
                app.enter_export_crop_mode(0);
                assert!(app.export_crop_mode);
            }
            NewEntry::ExportDialog => {
                app.open_export_dialog_for_current(&ctx, 0);
                assert!(
                    app.export_dialog.is_some(),
                    "Ctrl+E must reach the actual dialog"
                );
            }
        }
        assert!(!app.erase_mode, "{entry:?} starts before source completion");
        // Closing a different mode must not be forced by late source adoption.
        sender
            .send(crate::fs_animation::FsLoadResult::Static {
                ci: egui::ColorImage::filled([4, 6], egui::Color32::GREEN),
                source_dims: CANONICAL,
                animation: crate::fs_animation::StaticAnimationState::Still,
            })
            .unwrap();
        app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
        assert!(
            !app.erase_mode,
            "{entry:?} must exclude the obsolete erase entry"
        );
        assert!(app.erase_base_cache.is_empty());
        assert!(
            app.current_raw_source_pixels(0).is_some(),
            "hydration itself is still useful"
        );
        match entry {
            NewEntry::Analysis => assert!(app.analysis_mode),
            NewEntry::Text => assert!(app.text_mode),
            NewEntry::Crop => assert!(app.export_crop_mode),
            NewEntry::ExportDialog => assert!(app.export_dialog.is_some()),
        }
    }
}

fn original_with_margins(margin: usize) -> crate::fs_animation::FsLoadResult {
    let mut image = egui::ColorImage::filled([100, 150], egui::Color32::WHITE);
    let vertical = margin * 3 / 2;
    for y in vertical..150 - vertical {
        for x in margin..100 - margin {
            image[(x, y)] = egui::Color32::BLACK;
        }
    }
    crate::fs_animation::FsLoadResult::Static {
        ci: image,
        source_dims: [100, 150],
        animation: crate::fs_animation::StaticAnimationState::Still,
    }
}

#[test]
fn retained_material_entry_view_trim_single_detect_and_seed_wait_for_original() {
    use crate::ui_view_trim::OriginalViewTrimAction;
    use crate::view_trim::{ViewTrimApplyMode, ViewTrimMargins};

    let ctx = egui::Context::default();
    for action in [
        OriginalViewTrimAction::DetectBook,
        OriginalViewTrimAction::SeedPage,
    ] {
        let mut app = final_only_page(&ctx);
        app.adjustment_mode = crate::ui_helpers::MetadataPanelOpenState::ByPointer;
        app.view_trim_apply_mode = ViewTrimApplyMode::Book;
        app.view_trim_book_settings.enabled = true;
        app.view_trim_book_settings.single = ViewTrimMargins {
            left: 0.03,
            ..Default::default()
        };
        let before = app.view_trim_book_settings;
        let seed_id = egui::Id::unique(("view_trim_auto_page_seed_pending", 0usize));
        ctx.data_mut(|data| data.insert_temp(seed_id, true));
        let (sender, cancel) = pending_original(&mut app);
        app.request_view_trim_original_action(&ctx, 0, SpreadPair::Single, action);
        assert_eq!(app.view_trim_book_settings, before);
        assert!(
            app.view_trim_page_overrides.is_empty(),
            "missing raw must not become a zero page seed"
        );
        let pending = app.fs_pending.get(&0).unwrap();
        assert!(Arc::ptr_eq(&pending.cancel, &cancel));
        assert!(matches!(
            pending.purpose,
            crate::app::FsLoadPurpose::Original {
                entry: crate::app::OriginalMaterialEntry::ViewTrim { .. }
            }
        ));
        assert_eq!(ctx.data(|data| data.get_temp::<bool>(seed_id)), Some(true));
        sender.send(original_with_margins(20)).unwrap();
        app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
        match action {
            OriginalViewTrimAction::DetectBook => {
                assert!(app.view_trim_book_settings.single.left > 0.1)
            }
            OriginalViewTrimAction::SeedPage => {
                assert_eq!(app.view_trim_book_settings, before);
                let page = app.view_trim_page_overrides.get(&0).unwrap();
                assert!(page.enabled && page.margins.left > 0.1 && page.margins.top > 0.1);
            }
            OriginalViewTrimAction::DetectPage => unreachable!(),
        }
        assert_eq!(ctx.data(|data| data.get_temp::<bool>(seed_id)), Some(false));
        assert!(
            app.adjustment_mode.is_open(),
            "the controls' own panel must remain admissible"
        );
    }
}

#[test]
fn retained_material_entry_view_trim_spread_keeps_one_action_until_both_originals_arrive() {
    use crate::ui_view_trim::OriginalViewTrimAction;
    use crate::view_trim::ViewTrimApplyMode;

    let ctx = egui::Context::default();
    for action in [
        OriginalViewTrimAction::DetectBook,
        OriginalViewTrimAction::SeedPage,
    ] {
        for owner_finishes_first in [false, true] {
            let mut app = final_only_page(&ctx);
            app.fullscreen_idx = Some(21);
            app.spread_mode = SpreadMode::LtrCover;
            app.viewer_navigation_caches.invalidate();
            let pair = app.resolve_visible_spread_pair(21);
            let SpreadPair::Double { left, right } = pair else {
                panic!("final cover must be a displayed pair");
            };
            assert!(!app.fs_cache.contains_key(&left) && !app.fs_cache.contains_key(&right));
            app.view_trim_apply_mode = ViewTrimApplyMode::Book;
            app.view_trim_book_settings.enabled = true;
            let before = app.view_trim_book_settings;
            let (left_sender, _) = pending_original_at(&mut app, left);
            let (right_sender, _) = pending_original_at(&mut app, right);
            app.request_view_trim_original_action(&ctx, 21, pair, action);
            let count_actions = |app: &App| {
                app.fs_pending
                    .values()
                    .filter(|pending| {
                        matches!(
                            pending.purpose,
                            crate::app::FsLoadPurpose::Original {
                                entry: crate::app::OriginalMaterialEntry::ViewTrim { .. }
                            }
                        )
                    })
                    .count()
            };
            assert_eq!(count_actions(&app), 1);
            if owner_finishes_first {
                left_sender.send(original_with_margins(20)).unwrap();
            } else {
                right_sender.send(original_with_margins(10)).unwrap();
            }
            app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
            assert_eq!(
                count_actions(&app),
                1,
                "the same action must wait on the remaining raw source"
            );
            assert_eq!(
                app.view_trim_book_settings, before,
                "one-page arrival must not partly commit a spread"
            );
            assert!(app.view_trim_page_overrides.is_empty());
            if owner_finishes_first {
                right_sender.send(original_with_margins(10)).unwrap();
            } else {
                left_sender.send(original_with_margins(20)).unwrap();
            }
            app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
            assert_eq!(count_actions(&app), 0);
            match action {
                OriginalViewTrimAction::DetectBook => {
                    assert!(app.view_trim_book_settings.spread_linked.top > 0.05)
                }
                OriginalViewTrimAction::SeedPage => {
                    for idx in [left, right] {
                        let page = app.view_trim_page_overrides.get(&idx).unwrap();
                        assert!(page.enabled && page.margins.top > 0.05);
                    }
                    assert_eq!(app.view_trim_book_settings, before);
                }
                OriginalViewTrimAction::DetectPage => unreachable!(),
            }
            let completed_book = app.view_trim_book_settings;
            let completed_pages = app.view_trim_page_overrides.clone();
            app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
            assert_eq!(app.view_trim_book_settings, completed_book);
            assert_eq!(app.view_trim_page_overrides, completed_pages);
        }
    }
}

#[test]
fn retained_material_entry_view_trim_scope_or_settings_change_cancels_deferred_detection() {
    use crate::ui_view_trim::OriginalViewTrimAction;
    use crate::view_trim::{ViewTrimApplyMode, ViewTrimMargins, ViewTrimPageOverride};

    let ctx = egui::Context::default();
    for change_scope in [false, true] {
        let mut app = final_only_page(&ctx);
        app.view_trim_apply_mode = ViewTrimApplyMode::Book;
        let (sender, _) = pending_original(&mut app);
        app.request_view_trim_original_action(
            &ctx,
            0,
            SpreadPair::Single,
            OriginalViewTrimAction::DetectBook,
        );
        if change_scope {
            app.view_trim_page_overrides.insert(
                0,
                ViewTrimPageOverride::from_margins(ViewTrimMargins {
                    left: 0.04,
                    ..Default::default()
                }),
            );
        } else {
            app.view_trim_book_settings.single.left = 0.04;
        }
        let expected_book = app.view_trim_book_settings;
        let expected_pages = app.view_trim_page_overrides.clone();
        sender.send(original_with_margins(20)).unwrap();
        app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
        assert_eq!(app.view_trim_book_settings, expected_book);
        assert_eq!(app.view_trim_page_overrides, expected_pages);
        assert!(app.current_raw_source_pixels(0).is_some());
    }
}

#[test]
fn retained_material_entry_view_trim_simultaneous_spread_sources_commit_without_another_load() {
    use crate::ui_view_trim::OriginalViewTrimAction;
    use crate::view_trim::ViewTrimApplyMode;

    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    app.fullscreen_idx = Some(21);
    app.spread_mode = SpreadMode::LtrCover;
    app.viewer_navigation_caches.invalidate();
    let pair = app.resolve_visible_spread_pair(21);
    let SpreadPair::Double { left, right } = pair else {
        panic!("final cover displayed pair");
    };
    app.view_trim_apply_mode = ViewTrimApplyMode::Book;
    let (left_sender, _) = pending_original_at(&mut app, left);
    let (right_sender, _) = pending_original_at(&mut app, right);
    app.request_view_trim_original_action(&ctx, 21, pair, OriginalViewTrimAction::DetectBook);
    left_sender.send(original_with_margins(20)).unwrap();
    right_sender.send(original_with_margins(10)).unwrap();
    app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
    assert!(app.view_trim_book_settings.spread_linked.top > 0.05);
    assert!(!app.fs_pending.contains_key(&left) && !app.fs_pending.contains_key(&right));
    assert!(!app.fs_upload_backlog_contains(left) && !app.fs_upload_backlog_contains(right));
}

#[test]
fn retained_material_entry_view_trim_failed_partner_terminates_the_accepted_action() {
    use crate::ui_view_trim::OriginalViewTrimAction;
    use crate::view_trim::ViewTrimApplyMode;

    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    app.fullscreen_idx = Some(21);
    app.spread_mode = SpreadMode::LtrCover;
    app.viewer_navigation_caches.invalidate();
    let pair = app.resolve_visible_spread_pair(21);
    let SpreadPair::Double { left, right } = pair else {
        panic!("final cover displayed pair");
    };
    app.view_trim_apply_mode = ViewTrimApplyMode::Book;
    let before = app.view_trim_book_settings;
    let (left_sender, _) = pending_original_at(&mut app, left);
    let (right_sender, _) = pending_original_at(&mut app, right);
    app.request_view_trim_original_action(&ctx, 21, pair, OriginalViewTrimAction::SeedPage);
    right_sender
        .send(crate::fs_animation::FsLoadResult::Failed)
        .unwrap();
    app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
    left_sender.send(original_with_margins(20)).unwrap();
    app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
    assert!(matches!(
        app.fs_cache.get(&right),
        Some(FsCacheEntry::Failed)
    ));
    assert!(
        !app.fs_pending.contains_key(&right),
        "callback must not automatically retry its failed partner"
    );
    assert_eq!(app.view_trim_book_settings, before);
    assert!(app.view_trim_page_overrides.is_empty());
}

#[test]
fn retained_material_entry_view_trim_page_detection_preserves_page_scope() {
    use crate::ui_view_trim::OriginalViewTrimAction;
    use crate::view_trim::{ViewTrimApplyMode, ViewTrimMargins, ViewTrimPageOverride};

    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    app.view_trim_apply_mode = ViewTrimApplyMode::Book;
    app.view_trim_page_overrides.insert(
        0,
        ViewTrimPageOverride::from_margins(ViewTrimMargins {
            left: 0.02,
            ..Default::default()
        }),
    );
    let before_book = app.view_trim_book_settings;
    let (sender, _) = pending_original(&mut app);
    app.request_view_trim_original_action(
        &ctx,
        0,
        SpreadPair::Single,
        OriginalViewTrimAction::DetectPage,
    );
    sender.send(original_with_margins(20)).unwrap();
    app.poll_prefetch(&ctx, crate::app::PollPrefetchOrigin::TopLevel);
    assert!(app.view_trim_page_overrides.get(&0).unwrap().margins.left > 0.1);
    assert_eq!(app.view_trim_apply_mode, ViewTrimApplyMode::Book);
    assert_eq!(app.view_trim_book_settings, before_book);
}

#[test]
fn retained_material_entry_legacy_crop_is_preserved_by_capture_and_ctrl_e() {
    use crate::export_crop::{CropAspectMode, CropRect, CropSettings};

    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    let legacy = CropSettings {
        rect: CropRect {
            min_x: 300.0,
            min_y: 450.0,
            max_x: 900.0,
            max_y: 1350.0,
        },
        aspect_mode: CropAspectMode::Free,
        source_size: None,
    };
    app.export_crop_page_settings.insert(0, legacy);
    let work = app.prepare_capture_pixel_work(&ctx, 0).unwrap();
    let crate::capture::CapturePixelWork::Single(job) = &work else {
        panic!("single capture");
    };
    assert_eq!(
        job.crop,
        Some(CropRect {
            min_x: 2.0,
            min_y: 3.0,
            max_x: 6.0,
            max_y: 9.0
        })
    );
    let (_, width, height, _) = crate::capture::run_pixel_work(work).unwrap();
    assert_eq!((width, height), (4, 6));
    assert_eq!(
        app.export_crop_page_settings
            .get(&0)
            .unwrap()
            .valid_source_size(),
        Some(CANONICAL)
    );
    // Start from the unmigrated row again to exercise Ctrl+E's own snapshot route.
    app.export_crop_page_settings.insert(0, legacy);
    app.open_export_dialog_for_current(&ctx, 0);
    let crate::export_dialog::ExportComposite::Single(page) =
        &app.export_dialog.as_ref().unwrap().composite
    else {
        panic!("single export");
    };
    let crop = page
        .edits
        .export_crop
        .expect("legacy crop must survive Ctrl+E snapshot");
    assert_eq!(crop.valid_source_size(), Some(CANONICAL));
    assert_eq!(crop.rect, legacy.rect);
    assert_final_only(&app);
}

#[test]
fn retained_material_entry_view_trim_accepts_existing_raw_preview_pixels() {
    use crate::ui_view_trim::OriginalViewTrimAction;
    use crate::view_trim::ViewTrimApplyMode;

    let ctx = egui::Context::default();
    let mut app = final_only_page(&ctx);
    app.items[0] = GridItem::Image(app.tmp.path().join("margin-preview.nef"));
    app.final_composite_cache.clear();
    app.viewer_navigation_caches.invalidate();
    app.view_trim_apply_mode = ViewTrimApplyMode::Book;
    let crate::fs_animation::FsLoadResult::Static { ci, .. } = original_with_margins(20) else {
        unreachable!();
    };
    let pixels = Arc::new(ci);
    let tex = ctx.load_texture(
        "raw-margin-preview",
        Arc::clone(&pixels),
        egui::TextureOptions::LINEAR,
    );
    app.fs_cache.insert(
        0,
        FsCacheEntry::RawPreview {
            preview: Some(crate::fs_animation::RawPreviewTexture { tex, pixels }),
            developed_dims: [1200, 1800],
            load_seq: 123,
        },
    );
    assert!(
        !app.raw_edit_target_gate(0),
        "this preview is not a developed pixel-edit source"
    );
    app.request_view_trim_original_action(
        &ctx,
        0,
        SpreadPair::Single,
        OriginalViewTrimAction::DetectBook,
    );
    assert!(app.view_trim_book_settings.single.left > 0.1);
    assert!(
        app.fs_pending.is_empty(),
        "normalized margin detection needs no RAW development"
    );
}
