//! B-specific production entry and in-flight snapshot regressions.
use super::rejection::PreparedEditEvents;
use super::*;
use crate::app::final_prepare::tests::{
    WorkerHold, app, begin_registration_frame, wait_idle, wait_ready,
};

fn local(app: &mut App, idx: usize) {
    app.local_adjust_pages.insert(idx);
    app.local_adjust_page_layers.insert(
        idx,
        local_adjust_core::LocalAdjustmentLayers::new(vec![
            local_adjust_core::LocalAdjustmentLayer::new(
                "lifecycle",
                local_adjust_core::LocalMask::Full,
                local_adjust_core::LocalEffect::None,
            ),
        ]),
    );
}

fn annotation(app: &mut App, idx: usize) {
    app.comic_pages.insert(idx);
    app.comic_docs.insert(
        app.page_path_key(idx).unwrap(),
        vec![comic_core::AnnotationObject::new_bubble(
            1,
            (8.0, 8.0),
            comic_core::BubbleObject {
                shape: comic_core::BubbleShape::RoundRect {
                    half_w: 4.0,
                    half_h: 4.0,
                    corner_px: 0.0,
                },
                fill: Some(comic_core::Rgba::new(255, 0, 0, 255)),
                outline: comic_core::StrokeStyle {
                    width_px: 0.0,
                    ..Default::default()
                },
                auto_size: false,
                ..Default::default()
            },
        )],
    );
    app.comic_fonts.seed_for_test(
        Some(Arc::new(comic_core::FontSet::new())),
        Default::default(),
    );
    app.comic_font_registry_loaded = true;
}

fn computing(
    app: &mut App,
    ctx: &egui::Context,
) -> (WorkerHold, FinalCompositeKey, Arc<AtomicBool>) {
    app.prepare_final_demand_topologies();
    let unit = app
        .d3_final_prepare_unit_at(0, crate::ui_fullscreen::FinalUnitPosition::Next)
        .unwrap();
    let hold = WorkerHold::new();
    assert!(app.spawn_prepared_edit_job(ctx, 1, unit, &mut false));
    hold.entered();
    let key = *app
        .final_effect_pending
        .keys()
        .find(|key| key.edit_key.idx == 1)
        .unwrap();
    assert!(matches!(
        app.final_effect_pending[&key].state,
        FinalEffectState::Computing(_)
    ));
    let cancel = app.final_effect_pending[&key].cancel.clone();
    (hold, key, cancel)
}

#[test]
fn computing_snapshot_changes_reject_completed_b_before_any_publication() {
    for change in ["params", "source", "items", "local_generation", "layers"] {
        let trace = PreparedEditEvents::start();
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        local(&mut app, 1);
        annotation(&mut app, 1);
        let (hold, key, cancel) = computing(&mut app, &ctx);
        match change {
            "params" => app.adjustment_page_params.get_mut(&1).unwrap().brightness += 1.0,
            "source" => {
                let FsCacheEntry::Static { pixels, .. } = app.fs_cache.get_mut(&1).unwrap() else {
                    unreachable!()
                };
                *pixels = Arc::new((**pixels).clone());
            }
            "items" => app.items_generation = app.items_generation.wrapping_add(1),
            "local_generation" => *app.local_adjust_generation.entry(1).or_default() += 1,
            "layers" => {
                app.local_adjust_page_layers
                    .insert(1, local_adjust_core::LocalAdjustmentLayers::new(Vec::new()));
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                app.final_effect_pending[&key].state,
                FinalEffectState::Computing(_)
            ),
            "{change}"
        );
        hold.release();
        assert_eq!(wait_ready(&mut app, 1), key);
        app.resolve_prepared_edit_for_display(&ctx, 1);
        let events = trace.take();
        assert!(cancel.load(Ordering::Relaxed), "{change}: {events:?}");
        let expected = match change {
            "params" => PreparedEditReject::Params,
            "source" => PreparedEditReject::SourceArcChanged,
            "items" => PreparedEditReject::ItemsGeneration,
            "local_generation" => PreparedEditReject::EditGeneration,
            "layers" => PreparedEditReject::LayersChanged,
            _ => unreachable!(),
        };
        assert!(
            events.iter().any(|event| event.key == key
                && event.reason
                    == PreparedEditDisposition::Cancelled(PreparedEditCancel::Rejected(expected))),
            "{change}: {events:?}"
        );
        assert!(!app.final_effect_pending.contains_key(&key), "{change}");
        assert!(app.local_adjust_cache.is_empty(), "{change}");
        assert!(app.conceal_cache.is_empty(), "{change}");
        assert!(
            !app.edit_result_cache.contains_key(&key.edit_key),
            "{change}"
        );
        assert!(!app.final_composite_cache.contains_key(&key), "{change}");
        assert!(app.comic_cache.is_empty(), "{change}");
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0, "{change}");
    }
}

#[test]
fn computing_document_change_excludes_annotation_and_keeps_final_publication() {
    let trace = PreparedEditEvents::start();
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    local(&mut app, 1);
    annotation(&mut app, 1);
    let page = app.page_path_key(1).unwrap();
    let (hold, key, cancel) = computing(&mut app, &ctx);
    app.comic_docs.insert(page.clone(), Vec::new());
    hold.release();
    wait_ready(&mut app, 1);
    app.resolve_prepared_edit_for_display(&ctx, 1);
    let events = trace.take();
    assert!(!cancel.load(Ordering::Relaxed), "{events:?}");
    assert!(
        events.iter().any(|event| event.key == key
            && event.reason
                == PreparedEditDisposition::ComicExcluded(ComicExclusion::DocumentChanged)),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.reason, PreparedEditDisposition::Cancelled(_))),
        "{events:?}"
    );
    assert!(app.final_effect_pending.is_empty());
    assert!(app.local_adjust_is_materialized(1));
    assert!(app.final_composite_cache.contains_key(&key));
    assert!(app.comic_cache.is_empty());
    assert!(app.comic_docs[&page].is_empty());
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
}

fn pivot(app: &mut App, tool: PageEditTargetSwitchTool) -> PageEditTargetSwitchRequest {
    let pivot = PageEditSpreadPivot {
        saved_mode: crate::settings::SpreadMode::Ltr,
        pair: (0, 1),
        navigation_anchor_idx: 0,
    };
    app.fullscreen_idx = Some(0);
    match tool {
        PageEditTargetSwitchTool::Erase => {
            app.erase_mode = true;
            app.erase_spread_ctx = Some(pivot);
        }
        PageEditTargetSwitchTool::LocalAdjustment => {
            app.local_adjust_mode = true;
            app.local_adjust_spread_ctx = app.capture_local_page_edit_pivot(pivot);
        }
        PageEditTargetSwitchTool::Conceal => {
            app.conceal_mode = true;
            app.conceal_spread_ctx = Some(pivot);
        }
    }
    app.page_edit_target_switch_request(tool, 1).unwrap()
}

#[test]
fn actual_edit_switch_and_original_switch_resume_revoke_b_in_every_unpublished_state() {
    for tool in [
        PageEditTargetSwitchTool::Erase,
        PageEditTargetSwitchTool::LocalAdjustment,
        PageEditTargetSwitchTool::Conceal,
    ] {
        for resume in [false, true] {
            for state in ["computing", "ready", "registering"] {
                let trace = PreparedEditEvents::start();
                let ctx = egui::Context::default();
                let mut app = app(&ctx);
                local(&mut app, 1);
                annotation(&mut app, 1);
                let (hold, key, cancel) = computing(&mut app, &ctx);
                if state != "computing" {
                    hold.release();
                    wait_ready(&mut app, 1);
                    if state == "registering" {
                        begin_registration_frame(&mut app, 17);
                        app.adopt_prepared_edit(
                            &ctx,
                            key,
                            FinalPrepareAdoption::RootFinalized(
                                None,
                                crate::ui_fullscreen::FinalUnitPosition::Next,
                            ),
                            false,
                        );
                        assert!(matches!(
                            app.final_effect_pending[&key].state,
                            FinalEffectState::Registering(_)
                        ));
                    }
                }
                let request = pivot(&mut app, tool);
                if resume {
                    app.resume_original_material_entry(
                        &ctx,
                        1,
                        OriginalMaterialEntry::Switch { request },
                    );
                } else {
                    match tool {
                        PageEditTargetSwitchTool::Erase => {
                            app.switch_erase_target_in_spread(&ctx, 1)
                        }
                        PageEditTargetSwitchTool::LocalAdjustment => {
                            app.switch_local_adjust_target_in_spread(&ctx, 1)
                        }
                        PageEditTargetSwitchTool::Conceal => {
                            app.switch_conceal_target_in_spread(&ctx, 1)
                        }
                    }
                }
                let events = trace.take();
                assert!(
                    cancel.load(Ordering::Relaxed),
                    "{tool:?} resume={resume} state={state}: {events:?}"
                );
                assert!(
                    events.iter().any(|event| event.key == key
                        && event.reason
                            == PreparedEditDisposition::Cancelled(
                                PreparedEditCancel::EditingEntry
                            )),
                    "{tool:?} resume={resume} state={state}: {events:?}"
                );
                assert!(!app.final_effect_pending.contains_key(&key));
                assert!(!app.final_composite_cache.contains_key(&key));
                hold.release();
                wait_idle(&app);
                app.poll_final_effects(&ctx);
                assert!(
                    !app.final_composite_cache.contains_key(&key),
                    "cancelled B cannot resume an edit"
                );
                app.cancel_local_adjust_pending_for_idx(1);
            }
        }
    }
}

#[test]
fn actual_local_and_conceal_resume_materializers_revoke_b_before_ordinary_owner() {
    for conceal in [false, true] {
        let trace = PreparedEditEvents::start();
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let (key, cancel) = app.seed_prepared_edit_entry_test(&ctx, 1, false);
        let request = pivot(&mut app, PageEditTargetSwitchTool::Conceal);
        if conceal {
            app.maybe_start_conceal_materialize(
                1,
                ConcealMaterializePurpose::ResumeAfterDisplaySwitch { request },
            );
        } else {
            app.maybe_start_local_adjust_render_with_continuation(
                1,
                LocalMaterializeContinuation::ResumeAfterDisplaySwitchConceal { request },
            );
        }
        let events = trace.take();
        assert!(
            cancel.load(Ordering::Relaxed),
            "conceal={conceal}: {events:?}"
        );
        assert!(
            events.iter().any(|event| event.key == key
                && event.reason
                    == PreparedEditDisposition::Cancelled(PreparedEditCancel::EditingEntry)),
            "conceal={conceal}: {events:?}"
        );
        assert!(!app.final_effect_pending.contains_key(&key));
        assert!(!app.final_composite_cache.contains_key(&key));
        assert!(app.local_adjust_pending.contains_key(&1));
        app.cancel_local_adjust_pending_for_idx(1);
    }
}

#[test]
fn original_resume_variants_cancel_b_but_material_only_preserves_its_owner() {
    for entry in [
        OriginalMaterialEntry::Erase {
            requested_fs_idx: 1,
        },
        OriginalMaterialEntry::Conceal {
            requested_fs_idx: 1,
        },
        OriginalMaterialEntry::LocalAdjustment {
            requested_fs_idx: 1,
        },
        OriginalMaterialEntry::AutoCrop,
        OriginalMaterialEntry::ApplyEraseSlot {
            requested_fs_idx: 1,
            slot: 0,
        },
        OriginalMaterialEntry::MaterialOnly,
    ] {
        let trace = PreparedEditEvents::start();
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let (key, cancel) = app.seed_prepared_edit_entry_test(&ctx, 1, false);
        app.fullscreen_idx = Some(1);
        app.resume_original_material_entry(&ctx, 1, entry);
        let events = trace.take();
        if entry == OriginalMaterialEntry::MaterialOnly {
            assert!(!cancel.load(Ordering::Relaxed), "{entry:?}: {events:?}");
            assert!(
                app.final_effect_pending.contains_key(&key),
                "{entry:?}: {events:?}"
            );
            assert!(events.is_empty(), "{entry:?}: {events:?}");
            app.cancel_prepared_edit_for_entry(1);
        } else {
            assert!(cancel.load(Ordering::Relaxed), "{entry:?}: {events:?}");
            assert!(
                !app.final_effect_pending.contains_key(&key),
                "{entry:?}: {events:?}"
            );
            assert!(
                events.iter().any(|event| event.key == key
                    && event.reason
                        == PreparedEditDisposition::Cancelled(PreparedEditCancel::EditingEntry)),
                "{entry:?}: {events:?}"
            );
            assert!(
                !app.final_composite_cache.contains_key(&key),
                "{entry:?}: {events:?}"
            );
        }
        app.cancel_local_adjust_pending_for_idx(1);
    }
}

#[cfg(windows)]
#[test]
fn actual_detached_display_update_resolves_b_before_the_local_producer() {
    use crate::ui_fullscreen::page_turn_submission_tests::{frame, install_presentation};
    for state in ["computing", "ready", "registering"] {
        let trace = PreparedEditEvents::start();
        let ctx = egui::Context::default();
        install_presentation(&ctx);
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
        let mut app = app(&ctx);
        let window = 96_672;
        let owner_id =
            app.build_active_context_for_test(Some(window), DetachedSource::Image, |owner| {
                owner.items = (0..4)
                    .map(|idx| GridItem::Image(PathBuf::from(format!("C:/b-display/{idx}.png"))))
                    .collect();
                owner.visible_indices = vec![0, 1, 2, 3];
                owner.details_order = owner.visible_indices.clone();
                owner.thumbnails = vec![ThumbnailState::Pending; 4];
                owner.image_metas = vec![None; 4];
                owner.fullscreen_idx = Some(0);
                owner.spread_mode = crate::settings::SpreadMode::Single;
                owner.reading_flow = crate::settings::ReadingFlow::Paged;
                owner.viewer_presentation = ViewerPresentation::DetachedWindow;
                owner.detached_viewer_independent_active = true;
                owner.set_detached_window_binding_for_test(Some(window));
                for idx in 0..4 {
                    let pixels = Arc::new(egui::ColorImage::filled(
                        [80, 120],
                        egui::Color32::LIGHT_BLUE,
                    ));
                    let tex = ctx.load_texture(
                        format!("b-display-{idx}"),
                        pixels.clone(),
                        DISPLAY_IMAGE_TEXTURE_OPTIONS,
                    );
                    owner.fs_cache.insert(
                        idx,
                        FsCacheEntry::Static {
                            pdf_rerender_failures: Vec::new(),
                            pdf_metadata: None,
                            tex,
                            pixels,
                            source_dims: Some([80, 120]),
                            load_seq: 1,
                            animation: crate::fs_animation::StaticAnimationState::Still,
                        },
                    );
                    owner.record_page_dims_for_spread(idx, (80, 120));
                    let mut params = crate::adjustment::AdjustParams::default();
                    params.brightness = 15.0;
                    owner.adjustment_page_params.insert(idx, params);
                }
                let _ = owner.page_turn_prefetch_targets(0, 2, 2);
            });
        let (hold, key, cancel, expected, private_texture) = app
            .with_active_viewer_context(|owner| {
                owner.activate_mounted_as_independent_detached(window);
                local(owner, 1);
                annotation(owner, 1);
                let (hold, key, cancel) = computing(owner, &ctx);
                let mut expected = None;
                let mut private_texture = None;
                if state != "computing" {
                    hold.release();
                    wait_ready(owner, 1);
                    expected = Some(owner.prepared_edit_test_local_pixels(key));
                    if state == "registering" {
                        begin_registration_frame(owner, 19);
                        owner.adopt_prepared_edit(
                            &ctx,
                            key,
                            FinalPrepareAdoption::RootFinalized(
                                None,
                                crate::ui_fullscreen::FinalUnitPosition::Next,
                            ),
                            false,
                        );
                        let FinalEffectState::Registering(registration) =
                            &owner.final_effect_pending[&key].state
                        else {
                            panic!("private registration");
                        };
                        private_texture = Some(registration.final_texture.as_ref().unwrap().id());
                    }
                }
                owner.fullscreen_idx = Some(1);
                (hold, key, cancel, expected, private_texture)
            })
            .unwrap();
        frame(&ctx, |ui, _| {
            assert!(app.update_active_viewer_context(ui, None).updated);
        });
        let events = trace.take();
        app.with_active_viewer_context(|owner| {
            assert_eq!(owner.projected_viewer_context_id(), owner_id);
            assert!(!owner.final_effect_pending.contains_key(&key), "{state}");
            if let Some(expected) = &expected {
                assert!(!cancel.load(Ordering::Relaxed), "{state}: {events:?}");
                assert!(
                    !events.iter().any(|event| event.key == key
                        && matches!(event.reason, PreparedEditDisposition::Cancelled(_))),
                    "{state}: {events:?}"
                );
                assert!(
                    Arc::ptr_eq(&owner.current_local_adjust_pixels(1).unwrap(), expected),
                    "{state}"
                );
                assert!(
                    owner.local_adjust_pending.is_empty(),
                    "prepared result precedes detached local producer: {state}"
                );
                assert!(owner.final_composite_cache.contains_key(&key), "{state}");
                if let Some(texture) = private_texture {
                    assert_eq!(owner.final_composite_cache[&key].texture.id(), texture);
                }
            } else {
                assert!(cancel.load(Ordering::Relaxed), "{events:?}");
                assert!(
                    events.iter().any(|event| event.key == key
                        && event.reason
                            == PreparedEditDisposition::Cancelled(
                                PreparedEditCancel::DisplayComputing
                            )),
                    "{events:?}"
                );
                assert!(
                    owner.local_adjust_pending.contains_key(&1)
                        || owner.local_adjust_is_materialized(1),
                    "ordinary display proceeds after Computing cancellation"
                );
            }
            owner.cancel_local_adjust_pending_for_idx(1);
        })
        .unwrap();
        hold.release();
        wait_idle(&app);
        assert!(
            app.final_effect_pending.is_empty(),
            "main context never received detached ownership"
        );
        assert!(!app.final_composite_cache.contains_key(&key));
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    }
}
