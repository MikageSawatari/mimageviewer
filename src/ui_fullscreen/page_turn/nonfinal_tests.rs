//! Resident-quality acceptance must not manufacture a final image to start a turn.
#![cfg(test)]
use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, frame, ready_app};

#[test]
fn d3_first_offer_selects_registered_exact_finals_and_keeps_unregistered_pair_page_resident() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        for registered_pair in [false, true] {
            let ctx = egui::Context::default();
            install(&ctx);
            let mut app = ready_app(&ctx, mode);
            app.settings.page_turn_animation_enabled = true;
            app.settings.page_turn_animation_duration_ms = 600;
            frame(&ctx, |ui, ctx| {
                draw_current(&mut app, ui, ctx);
            });
            let target = if mode.is_spread() { 2 } else { 1 };
            let key = crate::app::final_prepare_tests::queued_final(&mut app, target);
            let other_key = (registered_pair && mode.is_spread())
                .then(|| crate::app::final_prepare_tests::queued_final(&mut app, target + 1));
            frame(&ctx, |_, ctx| {
                // This common decision checkpoint must adopt before resolving
                // navigation or capturing the first accepted resident material.
                app.fs_page_turn_decision_for_frame(ctx, 0);
                let operation = ViewerOperationId::Pad {
                    batch: next_operation_batch(),
                    ordinal: 0,
                };
                app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "d3-final");
                app.offer_page_turn_with_operation(ctx, target, true, Some(operation));
                let PresentationState::Offered { offer, .. } = &app.page_turn_presentation.state
                else {
                    panic!("offer: {:?}", take_test_events());
                };
                let accepted = &offer.ready_sources.0;
                let final_id = app.final_composite_cache.get(&key).unwrap().texture.id();
                assert_eq!(
                    accepted
                        .iter()
                        .find(|source| source.occurrence.idx == target)
                        .unwrap()
                        .resource
                        .source_texture_id(),
                    final_id
                );
                if let Some(key) = other_key {
                    assert_eq!(
                        accepted
                            .iter()
                            .find(|source| source.occurrence.idx == target + 1)
                            .unwrap()
                            .resource
                            .source_texture_id(),
                        app.final_composite_cache.get(&key).unwrap().texture.id()
                    );
                } else if mode.is_spread() {
                    assert!(
                        app.exact_complete_final_composite_texture(target + 1)
                            .is_none()
                    );
                    assert_eq!(
                        accepted
                            .iter()
                            .find(|source| source.occurrence.idx == target + 1)
                            .unwrap()
                            .resource
                            .source_texture_id(),
                        app.resolve_fs_display_tex(target + 1, false).unwrap().id()
                    );
                }
            });
        }
    }
}

fn install_resident_thumbnail(
    app: &mut App,
    ctx: &egui::Context,
    idx: usize,
) -> egui::TextureHandle {
    let texture = ctx.load_texture(
        format!("nonfinal-resident-thumb-{idx}"),
        egui::ColorImage::filled([8, 12], egui::Color32::from_rgb(180, 100, 40)),
        egui::TextureOptions::LINEAR,
    );
    app.thumbnails[idx] = ThumbnailState::loaded_raster(
        texture.clone(),
        crate::thumb_loader::ThumbLoadOrigin::UpgradeableCache,
        false,
        12,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((80, 120)),
        None,
    );
    texture
}

fn seed_only_outgoing(ctx: &egui::Context, app: &mut App) {
    install(ctx);
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.thumbnails = (0..app.items.len())
        .map(|_| ThumbnailState::Pending)
        .collect();
    frame(ctx, |ui, ctx| {
        draw_current(app, ui, ctx);
    });
    assert!(matches!(
        app.page_turn_presentation.state,
        PresentationState::Flat(_)
    ));
    assert!(
        app.edit_result_cache
            .get(&app.current_edit_result_key(1))
            .is_none()
    );
    assert!(
        !app.final_composite_cache
            .iter()
            .any(|(key, _)| key.edit_key.idx == 1)
    );
    take_test_events();
}

fn assert_acceptance_did_not_start_producers(app: &App, idx: usize) {
    assert!(
        app.edit_result_cache
            .get(&app.current_edit_result_key(idx))
            .is_none()
    );
    assert!(
        !app.final_composite_cache
            .iter()
            .any(|(key, _)| key.edit_key.idx == idx)
    );
    assert!(!app.fs_pending.contains_key(&idx));
    assert!(!app.local_adjust_pending.contains_key(&idx));
    assert!(!app.comic_bake_pending.contains_key(&idx));
    assert!(
        !app.final_effect_pending
            .keys()
            .any(|key| key.edit_key.idx == idx)
    );
    assert!(
        !app.final_ai_pending
            .keys()
            .any(|key| key.edit_key.idx == idx)
    );
    let key = app.page_path_key(idx).unwrap();
    assert!(!app.comic_docs.contains_key(&key));
}

fn accept_and_draw_resident_target(
    ctx: &egui::Context,
    app: &mut App,
    expected: &egui::TextureHandle,
    direct: bool,
    mut after_accept: impl FnMut(&mut App, &egui::Context),
) -> ViewerOperationId {
    let operation = ViewerOperationId::Pad {
        batch: next_operation_batch(),
        ordinal: 0,
    };
    frame(ctx, |ui, ctx| {
        app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "nonfinal-test");
        app.offer_page_turn_with_operation(ctx, 1, true, Some(operation));
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Offered { .. }
        ));
        assert_acceptance_did_not_start_producers(app, 1);
        after_accept(app, ctx);

        app.fullscreen_idx = Some(1);
        let (_, sources) = draw_current(app, ui, ctx);
        let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
            panic!("the normal target draw must submit the accepted resident unit")
        };
        assert!(
            receipt.curl,
            "resident quality must start the turn immediately"
        );
        assert_eq!(sources.pages.len(), 1);
        let resource = &sources.pages[0].resource;
        assert_eq!(resource.source_texture_id(), expected.id());
        assert_eq!(resource.source_generation().is_none(), direct);
    });
    let events = take_test_events();
    let events: Vec<_> = events
        .iter()
        .filter(|(fact, _, _)| fact.operation_id == operation)
        .collect();
    for kind in ["opportunity", "offered", "start"] {
        assert_eq!(
            events
                .iter()
                .filter(|(_, actual, _)| *actual == kind)
                .count(),
            1,
            "missing resident-quality event {kind}: {events:?}"
        );
    }
    assert!(!events.iter().any(|(_, kind, _)| *kind == "skip"));
    operation
}

#[test]
fn resident_static_page_starts_a_turn_without_acceptance_materialization() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    seed_only_outgoing(&ctx, &mut app);
    let FsCacheEntry::Static { tex, .. } = app.fs_cache.get(&1).unwrap() else {
        panic!()
    };
    let accepted = tex.clone();
    assert_eq!(
        app.page_turn_normal_ready_texture(1).unwrap().id(),
        accepted.id()
    );
    assert_acceptance_did_not_start_producers(&app, 1);
    accept_and_draw_resident_target(&ctx, &mut app, &accepted, false, |_, _| {});
}

#[test]
fn resident_thumbnail_starts_a_turn_without_acceptance_decode_or_upload() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    seed_only_outgoing(&ctx, &mut app);
    app.fs_cache.remove(&1);
    let accepted = install_resident_thumbnail(&mut app, &ctx, 1);
    assert_eq!(
        app.page_turn_normal_ready_texture(1).unwrap().id(),
        accepted.id()
    );
    assert_acceptance_did_not_start_producers(&app, 1);
    accept_and_draw_resident_target(&ctx, &mut app, &accepted, true, |_, _| {});
}

#[test]
fn replacing_the_resident_thumbnail_preserves_accepted_direct_resource_identity() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    seed_only_outgoing(&ctx, &mut app);
    app.fs_cache.remove(&1);
    let accepted = install_resident_thumbnail(&mut app, &ctx, 1);
    accept_and_draw_resident_target(&ctx, &mut app, &accepted, true, |app, ctx| {
        let replacement = install_resident_thumbnail(app, ctx, 1);
        assert_ne!(replacement.id(), accepted.id());
    });
}

#[test]
fn resident_incomplete_final_is_displayable_and_does_not_need_completion_at_accept() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    let expected = app.resolve_fs_processed_texture(&ctx, 1, false).unwrap();
    let final_key = app
        .final_composite_cache
        .iter()
        .find(|(key, entry)| key.edit_key.idx == 1 && entry.texture.id() == expected.id())
        .map(|(key, _)| *key)
        .unwrap();
    let entry = app.final_composite_cache.remove(&final_key).unwrap();
    app.final_composite_cache.insert(
        final_key,
        crate::app::FinalCompositeEntry {
            complete: false,
            ..entry
        },
    );
    let cache_len = app.final_composite_cache.len();
    assert_eq!(
        app.page_turn_normal_ready_texture(1).unwrap().id(),
        expected.id()
    );
    assert_eq!(app.final_composite_cache.len(), cache_len);
    assert!(!app.final_composite_cache.get(&final_key).unwrap().complete);
    assert!(app.final_effect_pending.is_empty());
    assert!(app.final_ai_pending.is_empty());
}

#[test]
fn colorize_guard_does_not_offer_unprocessed_static_or_catalog_thumbnail() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    app.thumbnails = (0..app.items.len())
        .map(|_| ThumbnailState::Pending)
        .collect();
    let _ = install_resident_thumbnail(&mut app, &ctx, 1);
    let mut params = app.effective_params(1).clone();
    params.colorize.mode = crate::colorize::ColorizeMode::AllImages;
    app.adjustment_page_params.insert(1, params);
    assert!(app.colorize_display_requires_final_effect(1));
    assert!(app.cached_passthrough_rendition(1).is_none());
    assert!(app.page_turn_normal_ready_texture(1).is_none());
    app.fs_cache.remove(&1);
    assert!(app.page_turn_normal_ready_texture(1).is_none());
    assert_acceptance_did_not_start_producers(&app, 1);
}

#[test]
fn creative_lut_guard_does_not_offer_an_unprocessed_resident_page() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    let lut = crate::creative_lut::BuiltinCreativeLut::WarmFilm;
    app.creative_lut_library = crate::creative_lut::CreativeLutLibrary::from_builtin_for_test(lut);
    let mut params = app.effective_params(1).clone();
    params.colorize.mode = crate::colorize::ColorizeMode::Disabled;
    params.creative_lut.id = Some(lut.id());
    params.creative_lut.strength = 1.0;
    app.adjustment_page_params.insert(1, params);
    assert!(app.colorize_display_requires_final_effect(1));
    assert!(app.page_turn_normal_ready_texture(1).is_none());
    assert_acceptance_did_not_start_producers(&app, 1);
}

fn publish_larger_raster_and_final(app: &mut App, ctx: &egui::Context) -> egui::TextureHandle {
    publish_larger_raster_and_final_for_idx(app, ctx, 1)
}

fn publish_larger_raster_and_final_for_idx(
    app: &mut App,
    ctx: &egui::Context,
    idx: usize,
) -> egui::TextureHandle {
    let color = match app.fs_cache.get(&idx) {
        Some(FsCacheEntry::Static { pixels, .. }) => pixels.pixels[0],
        _ => egui::Color32::from_rgb(180, 100, 40),
    };
    let pixels = Arc::new(egui::ColorImage::filled([160, 240], color));
    let texture = ctx.load_texture(
        format!("nonfinal-quality-upgrade-{idx}"),
        (*pixels).clone(),
        egui::TextureOptions::LINEAR,
    );
    app.fs_cache.insert(
        idx,
        FsCacheEntry::Static {
            pdf_rerender_failures: Vec::new(),
            pdf_metadata: None,
            tex: texture,
            pixels,
            source_dims: Some([160, 240]),
            load_seq: 2,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    app.bump_input_generation_for_fs_cache_reload(idx);
    app.ensure_final_composite_texture(ctx, idx).unwrap()
}

fn draw_and_assert_source(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    source: egui::TextureId,
    curl: bool,
) -> SubmittedUnitReceipt {
    let (_, sources) = draw_current(app, ui, ctx);
    let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
        panic!("the visible page must contribute a real paged submission")
    };
    assert_eq!(receipt.curl, curl);
    assert_eq!(sources.pages[0].resource.source_texture_id(), source);
    assert_eq!(receipt.pages()[0].1, source);
    receipt
}

#[test]
fn source_quality_publication_stays_frozen_until_canonical_flat_is_adopted() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    seed_only_outgoing(&ctx, &mut app);
    app.fs_cache.remove(&1);
    let accepted = install_resident_thumbnail(&mut app, &ctx, 1);
    accept_and_draw_resident_target(&ctx, &mut app, &accepted, true, |_, _| {});
    let tracker = match &app.page_turn_presentation.state {
        PresentationState::Turning { offer, .. } => offer.submission.clone(),
        _ => panic!("the adopted thumbnail curl must be Turning"),
    };
    assert_eq!(tracker.status(), SubmissionStatus::Started);

    let input_before = app.input_generation.get(&1).copied().unwrap_or(0);
    let published = publish_larger_raster_and_final(&mut app, &ctx);
    assert_ne!(published.id(), accepted.id());
    // Some publication routes use the broader source-generation invalidation.
    // That epoch alone must also remain a quality change for this same page.
    app.bump_input_generation(1);
    let upgraded = app.ensure_final_composite_texture(&ctx, 1).unwrap();
    assert_ne!(upgraded.id(), accepted.id());
    assert_ne!(
        app.input_generation.get(&1).copied().unwrap_or(0),
        input_before
    );
    app.thumbnails[1] = ThumbnailState::Evicted;
    assert_eq!(
        app.page_turn_held_resource(1).unwrap().source_texture_id(),
        accepted.id()
    );
    frame(&ctx, |ui, ctx| {
        let receipt = draw_and_assert_source(&mut app, ui, ctx, accepted.id(), true);
        assert!(
            !receipt.was_accepted(),
            "the output hook owns presentation adoption"
        );
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Turning { .. }
        ));
    });
    assert_eq!(tracker.status(), SubmissionStatus::Started);

    if let PresentationState::Turning { clock, offer, .. } = &mut app.page_turn_presentation.state {
        *clock = MotionClock::Running {
            start_at: Instant::now() - offer.duration - Duration::from_millis(1),
        };
    } else {
        panic!("a raster upgrade must preserve the turn until its deadline")
    }
    let mut canonical = None;
    frame(&ctx, |ui, ctx| {
        let receipt = draw_and_assert_source(&mut app, ui, ctx, accepted.id(), false);
        assert!(app.page_turn_completion_active());
        assert!(!app.commit_page_turn_continuation(ctx));
        assert_eq!(
            app.page_turn_held_resource(1).unwrap().source_texture_id(),
            accepted.id()
        );
        canonical = Some(receipt);
    });
    assert!(canonical.unwrap().was_accepted());
    assert_eq!(tracker.status(), SubmissionStatus::Finished);
    assert!(app.page_turn_completion_active());
    let PresentationState::Completing { incoming, .. } = &app.page_turn_presentation.state else {
        panic!()
    };
    assert!(incoming.reserved_bytes.load(Ordering::Relaxed) > 0);

    frame(&ctx, |ui, ctx| {
        // With no queued navigation this returns false, while retiring the
        // previous operation only after its prior output was adopted.
        assert!(!app.commit_page_turn_continuation(ctx));
        assert!(!app.page_turn_completion_active());
        let PresentationState::Flat(incoming) = &app.page_turn_presentation.state else {
            panic!()
        };
        assert_eq!(incoming.reserved_bytes.load(Ordering::Relaxed), 0);
        assert!(app.page_turn_held_resource(1).is_none());
        draw_and_assert_source(&mut app, ui, ctx, upgraded.id(), false);
    });
}

#[test]
fn interrupted_thumbnail_turn_submits_its_frozen_flat_before_next_navigation() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    seed_only_outgoing(&ctx, &mut app);
    app.fs_cache.remove(&1);
    let accepted = install_resident_thumbnail(&mut app, &ctx, 1);
    accept_and_draw_resident_target(&ctx, &mut app, &accepted, true, |_, _| {});
    let tracker = match &app.page_turn_presentation.state {
        PresentationState::Turning { offer, .. } => offer.submission.clone(),
        _ => panic!(),
    };
    let upgraded = publish_larger_raster_and_final(&mut app, &ctx);
    assert_ne!(upgraded.id(), accepted.id());
    let operation = ViewerOperationId::Pad {
        batch: next_operation_batch(),
        ordinal: 0,
    };
    let mut canonical = None;
    frame(&ctx, |ui, ctx| {
        assert!(app.page_turn_defer_navigation(
            ctx,
            2,
            None,
            crate::fs_page_load_scheduler::FsPageLoadContract::Sequential,
            Some(operation),
        ));
        assert_eq!(app.fullscreen_idx, Some(1));
        assert!(app.page_turn_completion_active());
        assert_eq!(tracker.status(), SubmissionStatus::Cancelled);
        canonical = Some(draw_and_assert_source(
            &mut app,
            ui,
            ctx,
            accepted.id(),
            false,
        ));
        assert!(!app.commit_page_turn_continuation(ctx));
        assert_eq!(app.fullscreen_idx, Some(1));
    });
    let canonical = canonical.unwrap();
    assert!(canonical.was_accepted());
    let canonical_pages = canonical.pages().to_vec();
    let held_material = |app: &App| {
        let PresentationState::Completing { incoming, .. } = &app.page_turn_presentation.state
        else {
            panic!("canonical incoming must remain held until continuation")
        };
        incoming
            .pages
            .iter()
            .map(|page| {
                (
                    page.resource.source_texture_id(),
                    page.resource.paint_texture_id(),
                    page.transform.paint_quad(),
                    page.transform.uv_rect,
                )
            })
            .collect::<Vec<_>>()
    };
    let canonical_material = held_material(&app);
    assert_eq!(app.fullscreen_idx, Some(1));
    // Both a quality publication for the held unit and the next landing final
    // arrive after canonical submission. Neither may rewrite accepted evidence.
    crate::app::final_prepare_tests::queued_final(&mut app, 1);
    let next_key = crate::app::final_prepare_tests::queued_final(&mut app, 2);
    frame(&ctx, |_, ctx| {
        crate::app::final_prepare_tests::expect_adopted_before_lookup(next_key, || {
            app.service_final_prepare_before_lookup(ctx);
            assert_eq!(held_material(&app), canonical_material);
            let PresentationState::Completing {
                completion: FlatCompletion::Submitted(receipt),
                ..
            } = &app.page_turn_presentation.state
            else {
                panic!("service must retain the submitted canonical receipt")
            };
            assert!(receipt.was_accepted());
            assert_eq!(receipt.pages(), canonical_pages.as_slice());
            assert!(app.commit_page_turn_continuation(ctx));
        });
        assert_eq!(app.fullscreen_idx, Some(2));
        assert_eq!(canonical.pages(), canonical_pages.as_slice());
        assert!(canonical.was_accepted());
        assert!(
            !app.page_turn_barrier_active(),
            "continuation remains instant"
        );
    });
}

#[test]
fn actual_edit_or_effective_adjustment_change_releases_and_cancels_the_frozen_turn() {
    for change_edit in [false, true] {
        let ctx = egui::Context::default();
        let mut app = ready_app(&ctx, SpreadMode::Single);
        seed_only_outgoing(&ctx, &mut app);
        app.fs_cache.remove(&1);
        let accepted = install_resident_thumbnail(&mut app, &ctx, 1);
        accept_and_draw_resident_target(&ctx, &mut app, &accepted, true, |_, _| {});
        let tracker = match &app.page_turn_presentation.state {
            PresentationState::Turning { offer, .. } => offer.submission.clone(),
            _ => panic!(),
        };
        publish_larger_raster_and_final(&mut app, &ctx);
        if change_edit {
            app.bump_erase_mask_generation(1);
        } else {
            let mut params = app.effective_params(1).clone();
            params.brightness += 10.0;
            app.adjustment_page_params.insert(1, params);
        }
        assert!(app.page_turn_held_resource(1).is_none());
        let changed = app.ensure_final_composite_texture(&ctx, 1).unwrap();
        frame(&ctx, |ui, ctx| {
            draw_and_assert_source(&mut app, ui, ctx, changed.id(), false);
            assert!(matches!(
                app.page_turn_presentation.state,
                PresentationState::Flat(_)
            ));
        });
        assert_eq!(tracker.status(), SubmissionStatus::Cancelled);
        assert!(
            take_test_events()
                .iter()
                .any(|(_, kind, _)| *kind == "cancel")
        );
    }
}

#[test]
fn missing_resident_source_emits_one_acceptance_skip_for_replayed_operation() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    seed_only_outgoing(&ctx, &mut app);
    app.fs_cache.remove(&1);
    assert!(!app.colorize_display_requires_final_effect(1));
    assert!(app.page_turn_resident_source(1).is_none());
    let operation = ViewerOperationId::Pad {
        batch: next_operation_batch(),
        ordinal: 0,
    };
    frame(&ctx, |_, ctx| {
        for _ in 0..3 {
            app.page_turn_external_operation(
                ctx,
                operation,
                OperationPhase::Press,
                "nonfinal-miss",
            );
            app.offer_page_turn_with_operation(ctx, 1, true, Some(operation));
        }
    });
    frame(&ctx, |_, ctx| {
        app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "nonfinal-miss");
        app.offer_page_turn_with_operation(ctx, 1, true, Some(operation));
    });
    let events = take_test_events();
    let skips: Vec<_> = events
        .iter()
        .filter(|(fact, kind, _)| fact.operation_id == operation && *kind == "skip")
        .collect();
    assert_eq!(
        skips.len(),
        1,
        "one accepted operation owns one reason: {skips:?}"
    );
    assert_eq!(skips[0].2, "no_displayable_source_at_accept");
    // A resident miss with the color/LUT guard disabled is the
    // no_resident_texture sub-check, rather than faithful_effect_not_resident.
    assert!(!app.colorize_display_requires_final_effect(1));
    assert_acceptance_did_not_start_producers(&app, 1);
}

#[test]
fn accepted_thumbnail_is_accounted_before_prepare_and_lease_released_after_cancel() {
    use crate::vram_budget::{VramAccountant, VramSubsystem};

    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    seed_only_outgoing(&ctx, &mut app);
    app.fs_cache.remove(&1);
    let accepted = install_resident_thumbnail(&mut app, &ctx, 1);
    let mut before = VramAccountant::default();
    app.page_turn_leases.account(&mut before);
    let before_bytes = before.finish().total_bytes();
    let mut incoming = VramAccountant::default();
    incoming.add_texture(VramSubsystem::PageTurnPresentation, &accepted, false);
    let incoming_bytes = incoming.finish().total_bytes();
    frame(&ctx, |_, ctx| {
        let operation = ViewerOperationId::Pad {
            batch: next_operation_batch(),
            ordinal: 0,
        };
        app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "nonfinal-account");
        app.offer_page_turn_with_operation(ctx, 1, true, Some(operation));
        let PresentationState::Offered { outgoing, offer } = &app.page_turn_presentation.state
        else {
            panic!("the incoming thumbnail is retained before its first normal prepare")
        };
        let weak_sources = Arc::downgrade(&offer.ready_sources);
        assert_eq!(
            offer.ready_sources.0[0].resource.source_texture_id(),
            accepted.id()
        );
        assert!(
            offer.ready_sources.0[0]
                .resource
                .source_generation()
                .is_none()
        );
        assert_eq!(offer.pin_bytes, before_bytes + incoming_bytes);
        let mut inventory = VramAccountant::default();
        app.page_turn_leases.account(&mut inventory);
        assert_eq!(
            inventory.finish().total_bytes(),
            before_bytes + incoming_bytes
        );

        let mut deduplicated = VramAccountant::default();
        // Ordinary owners and presentation owners share TextureIds. Outgoing
        // sources and the newly accepted incoming source must each count once.
        for page in &outgoing.pages {
            let texture = page.resource.source_texture().unwrap();
            let mipmapped = ctx
                .tex_manager()
                .read()
                .meta(texture.id())
                .unwrap()
                .options
                .mipmap_mode
                .is_some();
            deduplicated.add_texture(VramSubsystem::FinalCompositeCache, texture, mipmapped);
        }
        deduplicated.add_texture(VramSubsystem::Thumbnails, &accepted, false);
        app.page_turn_leases.account(&mut deduplicated);
        assert_eq!(
            deduplicated.finish().total_bytes(),
            before_bytes + incoming_bytes
        );
        assert_acceptance_did_not_start_producers(&app, 1);

        app.page_turn_presentation.cancel("nonfinal-account-cancel");
        assert!(weak_sources.upgrade().is_none());
        assert_eq!(app.page_turn_leases.reserved_bytes(), 0);
        let mut canceled = VramAccountant::default();
        app.page_turn_leases.account(&mut canceled);
        assert_eq!(canceled.finish().total_bytes(), before_bytes);
    });
}

fn arm_presenting_navigation(app: &mut App, composition: &SpreadDisplayComposition) {
    let anchor = composition.navigation_anchor_idx();
    let demand = crate::app::FsNavigationDisplayDemand::from_presentation(
        anchor,
        composition.pages_in_screen_order(app.spread_mode),
    )
    .unwrap();
    app.fs_nav_locked_gen = Some(app.items_generation);
    app.fs_holdover_tex = Some(FsHoldover::NavigationSequence(FsNavigationSequence {
        previous: None,
        chrome: FsNavigationChromeContinuation::None,
        purpose: FsNavigationPurpose::Ordinary,
        opened_at: Instant::now(),
        target: FsNavigationSequenceTarget::Display(FsNavigationDisplayTarget {
            items_generation: app.items_generation,
            anchor_idx: anchor,
            accept_rendition: false,
            phase: FsNavigationTargetPhase::Presenting {
                demand,
                presentation: FsNavigationPresentation::Materialized,
            },
        }),
    }));
}

#[test]
fn resampleable_quality_upgrade_retains_single_and_spread_sources_and_accepts_held_receipts() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        for invalidate_edit in [false, true] {
            let ctx = egui::Context::default();
            let mut app = ready_app(&ctx, mode);
            install(&ctx);
            app.settings.page_turn_animation_enabled = true;
            app.settings.page_turn_animation_duration_ms = 600;
            app.thumbnails = (0..app.items.len())
                .map(|_| ThumbnailState::Pending)
                .collect();
            frame(&ctx, |ui, ctx| {
                draw_current(&mut app, ui, ctx);
            });
            let target = if mode.is_spread() { 2 } else { 1 };
            let composition = app.spread_display_composition_for_anchor(target);
            let screen = composition.pages_in_screen_order(mode);
            let accepted: Vec<_> = screen
                .iter()
                .map(|page| {
                    let FsCacheEntry::Static { tex, .. } = app.fs_cache.get(&page.idx).unwrap()
                    else {
                        panic!()
                    };
                    (page.idx, tex.id())
                })
                .collect();
            frame(&ctx, |ui, ctx| {
                let operation = ViewerOperationId::Pad {
                    batch: next_operation_batch(),
                    ordinal: 0,
                };
                app.page_turn_external_operation(
                    ctx,
                    operation,
                    OperationPhase::Press,
                    "nonfinal-raw",
                );
                app.offer_page_turn_with_operation(ctx, target, true, Some(operation));
                app.fullscreen_idx = Some(target);
                let (_, sources) = draw_current(&mut app, ui, ctx);
                let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                    panic!()
                };
                assert!(receipt.curl);
                for (page, (idx, source)) in sources.pages.iter().zip(&accepted) {
                    assert_eq!(page.idx(), *idx);
                    assert_eq!(page.resource.source_texture_id(), *source);
                    assert_eq!(page.resource.source_generation().unwrap().input, 0);
                }
            });
            let tracker = match &app.page_turn_presentation.state {
                PresentationState::Turning { offer, .. } => offer.submission.clone(),
                _ => panic!(),
            };
            let upgraded: Vec<_> = accepted
                .iter()
                .map(|(idx, _)| {
                    (
                        *idx,
                        publish_larger_raster_and_final_for_idx(&mut app, &ctx, *idx).id(),
                    )
                })
                .collect();
            frame(&ctx, |ui, ctx| {
                let (_, sources) = draw_current(&mut app, ui, ctx);
                let FsDisplayUnitSubmission::Paged(receipt) = &sources.submission else {
                    panic!()
                };
                assert!(receipt.curl);
                for (page, (idx, source)) in sources.pages.iter().zip(&accepted) {
                    assert_eq!(page.resource.source_texture_id(), *source);
                    assert_eq!(page.resource.source_generation().unwrap().input, 0);
                    assert_ne!(app.input_generation.get(idx).copied().unwrap_or(0), 0);
                }
            });
            assert_eq!(tracker.status(), SubmissionStatus::Started);
            if let PresentationState::Turning { clock, offer, .. } =
                &mut app.page_turn_presentation.state
            {
                *clock = MotionClock::Running {
                    start_at: Instant::now() - offer.duration - Duration::from_millis(1),
                };
            } else {
                panic!("publication must keep the single/spread turn active")
            }
            let mut canonical = None;
            let mut staged_composition = None;
            frame(&ctx, |ui, ctx| {
                let (composition, sources) = draw_current(&mut app, ui, ctx);
                let FsDisplayUnitSubmission::Paged(receipt) = &sources.submission else {
                    panic!()
                };
                assert!(!receipt.curl);
                assert_eq!(receipt.pages().len(), accepted.len());
                for (page, (_, source)) in sources.pages.iter().zip(&accepted) {
                    assert_eq!(page.resource.source_texture_id(), *source);
                    assert_eq!(page.resource.source_generation().unwrap().input, 0);
                }
                app.emit_fs_page_turn_ready_for_display_unit(
                    ctx,
                    &composition,
                    FsPageTurnDecision::normal(),
                    &sources,
                );
                staged_composition = Some(composition);
                canonical = Some(receipt.clone());
            });
            assert!(canonical.unwrap().was_accepted());
            assert_eq!(tracker.status(), SubmissionStatus::Finished);
            assert!(app.pending_fs_page_turn_ready.is_some());
            arm_presenting_navigation(&mut app, &staged_composition.unwrap());
            if invalidate_edit {
                app.bump_erase_mask_generation(target);
                assert!(app.page_turn_held_resource(target).is_none());
            }
            frame(&ctx, |_, ctx| {
                app.flush_submitted_page_turn_ready(ctx);
                assert_eq!(
                    app.fs_navigation_sequence_blocks_new_target(),
                    invalidate_edit
                );
                assert_eq!(app.fs_nav_locked_gen.is_some(), invalidate_edit);
            });
            assert!(app.pending_fs_page_turn_ready.is_none());
            if !invalidate_edit {
                frame(&ctx, |ui, ctx| {
                    app.commit_page_turn_continuation(ctx);
                    for (idx, _) in &accepted {
                        assert!(app.page_turn_held_resource(*idx).is_none());
                    }
                    let (_, sources) = draw_current(&mut app, ui, ctx);
                    let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                        panic!()
                    };
                    assert!(!receipt.curl);
                    for (page, (idx, source)) in sources.pages.iter().zip(&upgraded) {
                        assert_eq!(page.idx(), *idx);
                        assert_eq!(page.resource.source_texture_id(), *source);
                    }
                });
            }
        }
    }
}

#[test]
fn held_thumbnail_yields_to_original_preview_and_ineligible_editor_views() {
    let ctx = egui::Context::default();
    let mut app = ready_app(&ctx, SpreadMode::Single);
    seed_only_outgoing(&ctx, &mut app);
    app.fs_cache.remove(&1);
    let accepted = install_resident_thumbnail(&mut app, &ctx, 1);
    accept_and_draw_resident_target(&ctx, &mut app, &accepted, true, |_, _| {});
    publish_larger_raster_and_final(&mut app, &ctx);
    let original = match app.fs_cache.get(&1).unwrap() {
        FsCacheEntry::Static { tex, .. } => tex.clone(),
        _ => panic!(),
    };
    assert_ne!(original.id(), accepted.id());
    assert_eq!(
        app.page_turn_held_resource(1).unwrap().source_texture_id(),
        accepted.id()
    );

    let preview_resource = app.resolve_fs_processed_material(&ctx, 1, true).unwrap();
    assert_eq!(preview_resource.source_texture_id(), original.id());
    assert_eq!(preview_resource.source_texture_id(), original.id());
    assert!(preview_resource.source_generation().is_some());
    let mut preview_page = match &app.page_turn_presentation.state {
        PresentationState::Turning { incoming, .. } => incoming.pages[0].clone(),
        _ => panic!("original preview must not replace the presentation owner itself"),
    };
    preview_page.resource = preview_resource.into();
    preview_page.live = false;
    let prepared = app.page_turn_bind_prepared(preview_page);
    assert!(!prepared.live);
    assert_eq!(prepared.resource.source_texture_id(), original.id());

    app.erase_mode = true;
    assert!(app.is_overlay_edit_mode_active());
    assert!(app.page_turn_held_resource(1).is_none());
    app.erase_mode = false;
    assert!(app.page_turn_held_resource(1).is_some());

    app.analysis_mode = true;
    assert!(app.page_turn_held_resource(1).is_none());
    assert_eq!(
        app.resolve_fs_processed_texture(&ctx, 1, false)
            .unwrap()
            .id(),
        original.id()
    );
    app.analysis_mode = false;
    assert!(app.page_turn_held_resource(1).is_some());

    app.fs_zoom_active = true;
    assert!(app.page_turn_held_resource(1).is_none());
    app.fs_zoom_active = false;
    app.fs_zoom = 2.0;
    assert!(app.page_turn_held_resource(1).is_none());
    app.fs_zoom = 1.0;
    assert_eq!(
        app.page_turn_held_resource(1).unwrap().source_texture_id(),
        accepted.id()
    );
}
