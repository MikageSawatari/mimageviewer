//! Measured allocation sizes go through the real offer and prepared-unit recheck.
use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, frame, ready_app};
use crate::vram_budget::{VramAccountant, VramBudgetMode, VramPoolBudget};

// No GPU upload or high-resolution pixel allocation: this fixture exercises
// ownership/accounting/state transitions. Actual native rendering is covered by roi_tests.
fn metadata_texture(ctx: &egui::Context, idx: usize, size: [usize; 2]) -> egui::TextureHandle {
    let manager = ctx.tex_manager();
    let image = egui::ColorImage {
        size,
        source_size: egui::vec2(size[0] as f32, size[1] as f32),
        pixels: Vec::new(),
    };
    let id = manager.write().alloc(
        format!("measured-source-{idx}"),
        egui::ImageData::Color(Arc::new(image)),
        egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear)),
    );
    egui::TextureHandle::new(manager, id)
}

fn measured_offer(source_size: [usize; 2], output_size: [u32; 2], old_bytes: u64, expired: bool) {
    let ctx = egui::Context::default();
    install(&ctx);
    let budget = VramPoolBudget::from_pool_bytes(Some(12_655_263_744), VramBudgetMode::Fullscreen);
    assert_eq!(page_turn_pin_limit(budget), 1_012_421_099);
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("page_turn_test_budget"), budget));
    let mut app = ready_app(&ctx, SpreadMode::Ltr);
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 200;
    app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
    let mut templates = Vec::new();
    for anchor in [0, 2] {
        app.fullscreen_idx = Some(anchor);
        frame(&ctx, |ui, ctx| {
            draw_current(&mut app, ui, ctx);
        });
        let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
            panic!("ordinary prepared template");
        };
        assert_eq!(unit.pages.len(), 2);
        templates.push(unit.pages.clone());
    }
    app.final_composite_cache.clear();
    app.edit_result_cache.clear();
    let mut source_ids = Vec::new();
    for pages in &mut templates {
        for page in pages {
            let idx = page.occurrence.idx;
            let texture = metadata_texture(&ctx, idx, source_size);
            source_ids.push(texture.id());
            app.fs_cache.insert(
                idx,
                FsCacheEntry::Static {
                    tex: texture.clone(),
                    pixels: Arc::new(egui::ColorImage::filled([1, 1], egui::Color32::WHITE)),
                    source_dims: Some(source_size),
                    load_seq: 1,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
            page.resource = crate::gpu_lanczos::test_native_resource(
                app.fullscreen_paint_resource_for_texture(idx, texture),
                output_size,
            )
            .into();
        }
    }
    let output_bytes = output_size[0] as u64 * output_size[1] as u64 * 4;
    let source_bytes = crate::vram_budget::texture_size_texels(source_size, true) as u64 * 4;
    assert_eq!(4 * source_bytes + 2 * output_bytes, old_bytes);
    assert!(old_bytes > page_turn_pin_limit(budget));

    // Adopt the actual Native outgoing flat, through the production submit/hook.
    app.fullscreen_idx = Some(0);
    frame(&ctx, |_, ctx| {
        app.submit_paged_display_unit(ctx, &templates[0], None);
    });
    let PresentationState::Flat(outgoing) = &app.page_turn_presentation.state else {
        panic!("flat")
    };
    assert!(outgoing.evidence.was_accepted());
    assert!(
        outgoing
            .pages
            .iter()
            .all(|p| p.resource.source_texture().is_none())
    );
    assert_eq!(outgoing.retained_bytes(), 2 * output_bytes);
    take_test_events();
    let operation = ViewerOperationId::Pad {
        batch: next_operation_batch(),
        ordinal: 0,
    };
    frame(&ctx, |_, ctx| {
        app.page_turn_external_operation(
            ctx,
            operation,
            OperationPhase::Press,
            "measured-material",
        );
        app.offer_page_turn_with_operation(ctx, 2, true, Some(operation));
        let PresentationState::Offered { offer, .. } = &app.page_turn_presentation.state else {
            panic!(
                "source pin rejection must disappear: {:?}",
                take_test_events()
            );
        };
        assert_eq!(offer.pin_bytes, 2 * source_bytes + 2 * output_bytes);
        let accepted_at = offer.accepted_at;
        let duration = offer.duration;
        app.fullscreen_idx = Some(2);
        // This clock seam is at the unchanged accepted_at expiry test. A long
        // test machine stall cannot confuse pin acceptance with expiry/finish.
        let now = accepted_at
            + if expired {
                duration
            } else {
                Duration::from_millis(1)
            };
        let receipt = app
            .submit_paged_display_unit_with_clock(ctx, &templates[1], None, || now)
            .unwrap();
        assert_eq!(receipt.curl, !expired);
        if expired {
            assert!(matches!(
                app.page_turn_presentation.state,
                PresentationState::Flat(_)
            ));
        } else {
            let PresentationState::Turning {
                offer, incoming, ..
            } = &app.page_turn_presentation.state
            else {
                panic!("prepared allocation must pass pin recheck");
            };
            assert!(
                offer
                    .ready_sources
                    .0
                    .iter()
                    .all(|s| s.resource.source_texture().is_none())
            );
            assert!(
                incoming
                    .pages
                    .iter()
                    .all(|p| p.resource.source_texture().is_none())
            );
            assert_eq!(app.page_turn_leases.reserved_bytes(), 4 * output_bytes);
        }
    });
    let events = take_test_events();
    assert!(
        !events.iter().any(|(_, _, reason)| *reason == "pin_budget"),
        "{events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|(_, kind, _)| *kind == "offered")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|(_, kind, _)| *kind == "start")
            .count(),
        usize::from(!expired)
    );
    assert_eq!(
        events
            .iter()
            .filter(|(_, kind, _)| *kind == "finish")
            .count(),
        0
    );
    assert_eq!(
        events
            .iter()
            .filter(|(_, _, reason)| *reason == "offer_mismatch_or_expired")
            .count(),
        usize::from(expired)
    );
    drop(templates);
    app.fs_cache.clear();
    // Native presentation and its accepted diagnostics cannot keep raw sources.
    for id in source_ids {
        assert!(
            ctx.tex_manager().read().meta(id).is_none(),
            "source {id:?} still pinned"
        );
    }
    // These measured output dimensions deliberately model the recorded 03/04
    // allocations, not this 600x400 fixture's view demand. They prove ownership
    // and pin admission only. The demand-matched fixture below checks actual
    // source-free selection/preparation/drawing after eviction.
    let mut accountant = VramAccountant::default();
    app.page_turn_leases.account(&mut accountant);
    assert_eq!(
        accountant.finish().total_bytes(),
        if expired { 2 } else { 4 } * output_bytes
    );
}

#[test]
fn demand_matched_native_materials_survive_eviction_through_real_next_frame_draw() {
    let ctx = egui::Context::default();
    install(&ctx);
    let mut app = ready_app(&ctx, SpreadMode::Ltr);
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.thumbnails = (0..4).map(|_| ThumbnailState::Pending).collect();
    let mut templates = Vec::new();
    let mut source_ids = Vec::new();
    for anchor in [0, 2] {
        app.fullscreen_idx = Some(anchor);
        frame(&ctx, |ui, ctx| {
            draw_current(&mut app, ui, ctx);
        });
        let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
            panic!("ordinary prepared template");
        };
        let mut pages = unit.pages.clone();
        assert_eq!(pages.len(), 2);
        for page in &mut pages {
            let crate::gpu_lanczos::FullscreenPaintPayload::PresentationMaterial(material) =
                &page.resource
            else {
                panic!("ordinary adopted material");
            };
            let source = material.source_texture().unwrap().clone();
            source_ids.push(source.id());
            let request = page
                .transform
                .visible_region_request(page.painter.clip_rect())
                .unwrap();
            assert!(page.transform.total_scale * page.transform.pixels_per_point > 1.0);
            assert_eq!(
                app.effective_params(page.occurrence.idx).post_filter,
                crate::adjustment::PostFilter::None
            );
            let normal = app.fullscreen_paint_resource_for_texture(page.occurrence.idx, source);
            let native = crate::gpu_lanczos::test_native_resource_with_region(
                normal,
                request.target_size,
                request.source_uv_rect,
            );
            let captured = native.capture_presentation(&ctx);
            assert!(captured.matches_prepare(
                page.transform.total_scale,
                page.transform.pixels_per_point,
                Some(request),
                crate::adjustment::PostFilter::None,
                app.settings.downscale_smoothing_percent,
                app.settings.anime_upscale_source_limit,
            ));
            page.resource = native.into();
        }
        templates.push(pages);
    }
    app.fullscreen_idx = Some(0);
    frame(&ctx, |_, ctx| {
        app.submit_paged_display_unit(ctx, &templates[0], None);
    });
    frame(&ctx, |_, ctx| {
        let operation = ViewerOperationId::Pad {
            batch: next_operation_batch(),
            ordinal: 0,
        };
        app.page_turn_external_operation(
            ctx,
            operation,
            OperationPhase::Press,
            "demand-matched-material",
        );
        app.offer_page_turn_with_operation(ctx, 2, true, Some(operation));
        let PresentationState::Offered { offer, .. } = &app.page_turn_presentation.state else {
            panic!("resident input offer");
        };
        let now = offer.accepted_at + Duration::from_millis(1);
        app.fullscreen_idx = Some(2);
        let receipt = app
            .submit_paged_display_unit_with_clock(ctx, &templates[1], None, || now)
            .unwrap();
        assert!(receipt.curl);
    });
    let PresentationState::Turning { incoming, .. } = &app.page_turn_presentation.state else {
        panic!("prepared Native turn");
    };
    let native_ids = incoming
        .pages
        .iter()
        .map(|page| page.resource.paint_texture_id())
        .collect::<Vec<_>>();
    drop(templates);
    app.fs_cache.clear();
    app.final_composite_cache.clear();
    app.edit_result_cache.clear();
    for id in source_ids {
        assert!(
            ctx.tex_manager().read().meta(id).is_none(),
            "source {id:?} still pinned"
        );
    }
    if let PresentationState::Turning { offer, .. } = &mut app.page_turn_presentation.state {
        offer.accepted_at = Instant::now();
        offer.duration = Duration::from_millis(600);
    }
    app.fs_lanczos_cache.take_test_resolve_input_ids();
    let output = frame(&ctx, |ui, ctx| {
        let (_, sources) = draw_current(&mut app, ui, ctx);
        let FsDisplayUnitSubmission::Paged(receipt) = &sources.submission else {
            panic!("source-free Native receipt");
        };
        assert!(
            receipt.curl,
            "matching source-free Native must paint the frozen turn"
        );
        assert_eq!(sources.pages.len(), 2);
        for (page, id) in sources.pages.iter().zip(&native_ids) {
            assert!(page.resource.source_texture().is_none());
            assert_eq!(page.resource.paint_texture_id(), *id);
        }
        assert!(
            app.fs_lanczos_cache
                .take_test_resolve_input_ids()
                .is_empty()
        );
    });
    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    assert!(
        primitives.iter().any(|primitive| matches!(
            &primitive.primitive,
            egui::epaint::Primitive::Mesh(mesh) if native_ids.contains(&mesh.texture_id)
        )),
        "accepted Native material must reach the actual tessellated draw batch"
    );
}

#[test]
fn measured_03_04_native_materials_pass_real_offer_and_prepared_pin_recheck() {
    measured_offer([6144, 8192], [1620, 2160], 1_101_735_408, false);
    measured_offer([8000, 8192], [1919, 1965], 1_428_267_016, false);
}

#[test]
fn measured_native_pin_acceptance_does_not_change_legacy_offer_expiry() {
    measured_offer([6144, 8192], [1620, 2160], 1_101_735_408, true);
}

#[test]
fn real_offer_and_prepared_pin_boundaries_admit_equality_and_reject_excess() {
    for prepared in [false, true] {
        for margin in [-1_i64, 0, 1] {
            let ctx = egui::Context::default();
            install(&ctx);
            let mut app = ready_app(&ctx, SpreadMode::Single);
            app.settings.page_turn_animation_enabled = true;
            app.settings.page_turn_animation_duration_ms = 200;
            let mut incoming = Vec::new();
            for idx in [1, 0] {
                app.fullscreen_idx = Some(idx);
                frame(&ctx, |ui, ctx| {
                    draw_current(&mut app, ui, ctx);
                });
                if idx == 1 {
                    let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
                        panic!("flat")
                    };
                    incoming = unit.pages.clone();
                }
            }
            let PresentationState::Flat(outgoing) = &app.page_turn_presentation.state else {
                panic!("flat")
            };
            let outgoing_bytes = outgoing.retained_bytes();
            let (source, _) = app.page_turn_resident_source(1).unwrap();
            let mip = ctx
                .tex_manager()
                .read()
                .meta(source.id())
                .unwrap()
                .options
                .mipmap_mode
                .is_some();
            let input_bytes =
                crate::vram_budget::texture_size_texels(source.size(), mip) as u64 * 4;
            let output_bytes = 40 * 60 * 4;
            let pin_bytes = outgoing_bytes + if prepared { output_bytes } else { input_bytes };
            let limit = (pin_bytes as i64 + margin) as u64;
            let budget = VramPoolBudget::from_pool_bytes(
                Some((limit * 100).div_ceil(8)),
                VramBudgetMode::Fullscreen,
            );
            assert_eq!(page_turn_pin_limit(budget), limit);
            if !prepared {
                ctx.data_mut(|data| {
                    data.insert_temp(egui::Id::new("page_turn_test_budget"), budget)
                });
            } else {
                ctx.data_mut(|data| {
                    data.insert_temp(
                        egui::Id::new("page_turn_test_budget"),
                        VramPoolBudget::from_pool_bytes(
                            Some(1024 * 1024 * 1024),
                            VramBudgetMode::Fullscreen,
                        ),
                    )
                });
            }
            frame(&ctx, |_, ctx| {
                let operation = ViewerOperationId::Pad {
                    batch: next_operation_batch(),
                    ordinal: 0,
                };
                app.page_turn_external_operation(
                    ctx,
                    operation,
                    OperationPhase::Press,
                    "pin-boundary",
                );
                app.offer_page_turn_with_operation(ctx, 1, true, Some(operation));
                if !prepared {
                    assert_eq!(
                        matches!(
                            app.page_turn_presentation.state,
                            PresentationState::Offered { .. }
                        ),
                        margin >= 0,
                        "offer margin={margin}"
                    );
                    return;
                }
                let PresentationState::Offered { offer, .. } = &app.page_turn_presentation.state
                else {
                    panic!("offer")
                };
                let now = offer.accepted_at + Duration::from_millis(1);
                let crate::gpu_lanczos::FullscreenPaintPayload::NormalResource(source) =
                    &offer.ready_sources.0[0].resource
                else {
                    panic!("input")
                };
                incoming[0].resource =
                    crate::gpu_lanczos::test_native_resource(source.clone(), [40, 60]).into();
                ctx.data_mut(|data| {
                    data.insert_temp(egui::Id::new("page_turn_test_budget"), budget)
                });
                app.fullscreen_idx = Some(1);
                let receipt = app
                    .submit_paged_display_unit_with_clock(ctx, &incoming, None, || now)
                    .unwrap();
                assert_eq!(receipt.curl, margin >= 0, "prepared margin={margin}");
            });
        }
    }
}

#[test]
fn prepared_pin_recheck_rejects_output_that_exceeds_budget_after_input_release() {
    let ctx = egui::Context::default();
    install(&ctx);
    let budget =
        VramPoolBudget::from_pool_bytes(Some(16 * 1024 * 1024), VramBudgetMode::Fullscreen);
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("page_turn_test_budget"), budget));
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 200;
    let mut incoming = Vec::new();
    for idx in [1, 0] {
        app.fullscreen_idx = Some(idx);
        frame(&ctx, |ui, ctx| {
            draw_current(&mut app, ui, ctx);
        });
        if idx == 1 {
            let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
                panic!("flat")
            };
            incoming = unit.pages.clone();
        }
    }
    take_test_events();
    frame(&ctx, |_, ctx| {
        let operation = ViewerOperationId::Pad {
            batch: next_operation_batch(),
            ordinal: 0,
        };
        app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "output-budget");
        app.offer_page_turn_with_operation(ctx, 1, true, Some(operation));
        let PresentationState::Offered { offer, .. } = &app.page_turn_presentation.state else {
            panic!("offer")
        };
        assert!(offer.pin_bytes < page_turn_pin_limit(budget));
        let now = offer.accepted_at + Duration::from_millis(1);
        let crate::gpu_lanczos::FullscreenPaintPayload::NormalResource(source) =
            &offer.ready_sources.0[0].resource
        else {
            panic!("input lease")
        };
        incoming[0].resource =
            crate::gpu_lanczos::test_native_resource(source.clone(), [4096, 4096]).into();
        app.fullscreen_idx = Some(1);
        let receipt = app
            .submit_paged_display_unit_with_clock(ctx, &incoming, None, || now)
            .unwrap();
        assert!(
            !receipt.curl,
            "prepared output must be checked against the same budget"
        );
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Flat(_)
        ));
    });
    let events = take_test_events();
    assert_eq!(
        events
            .iter()
            .filter(|(_, kind, reason)| *kind == "skip" && *reason == "pin_budget")
            .count(),
        1
    );
    assert!(!events.iter().any(|(_, kind, _)| *kind == "start"));
}
