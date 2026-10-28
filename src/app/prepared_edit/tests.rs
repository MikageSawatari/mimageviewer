//! Deterministic CPU completion, publication, and owner-boundary regressions.
use super::*;
use crate::app::final_prepare::tests::{app, begin_registration_frame};

impl App {
    pub(crate) fn prepared_edit_test_local_pixels(
        &self,
        key: FinalCompositeKey,
    ) -> Arc<egui::ColorImage> {
        let ready = match &self.final_effect_pending[&key].state {
            FinalEffectState::CpuReady(FinalEffectReady::Edited(ready)) => ready,
            FinalEffectState::Registering(registration) => &registration.ready,
            _ => panic!("ready B test owner"),
        };
        ready
            .output
            .as_ref()
            .unwrap()
            .local
            .as_ref()
            .unwrap()
            .pixels
            .clone()
    }
    pub(crate) fn seed_prepared_edit_entry_test(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        publish: bool,
    ) -> (FinalCompositeKey, Arc<AtomicBool>) {
        local(self, idx, true);
        let input = self.prepared_edit_input(idx).unwrap_or_else(|| panic!(
            "B fixture idx={idx}: raw={} lower={} local_materialized={} final_pending={:?} local_pending={:?}",
            self.current_raw_source_pixels(idx).is_some(), self.current_local_adjust_source_pixels(idx).is_some(),
            self.local_adjust_is_materialized(idx), self.final_effect_pending.keys().collect::<Vec<_>>(),
            self.local_adjust_pending.keys().collect::<Vec<_>>()));
        let key = ready(self, ctx, input);
        let cancel = self.final_effect_pending[&key].cancel.clone();
        if publish {
            self.resolve_prepared_edit_for_display(ctx, idx);
        }
        (key, cancel)
    }
}

fn local(app: &mut App, idx: usize, active: bool) {
    let mut layer = local_adjust_core::LocalAdjustmentLayer::new(
        "prepared",
        local_adjust_core::LocalMask::Full,
        local_adjust_core::LocalEffect::None,
    );
    layer.enabled = active;
    app.local_adjust_pages.insert(idx);
    app.local_adjust_page_layers.insert(
        idx,
        local_adjust_core::LocalAdjustmentLayers::new(vec![layer]),
    );
}

fn bubble() -> Vec<comic_core::AnnotationObject> {
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
    )]
}

fn missing_font_doc() -> Vec<comic_core::AnnotationObject> {
    vec![comic_core::AnnotationObject::new_text(
        2,
        (2.0, 2.0),
        comic_core::TextBlock {
            text: "missing".to_owned(),
            font_key: "not-loaded".to_owned(),
            ..Default::default()
        },
    )]
}

fn annotation(app: &mut App, idx: usize) {
    app.comic_pages.insert(idx);
    app.comic_docs
        .insert(app.page_path_key(idx).unwrap(), bubble());
    // Same font-free filled-bubble fixture as composite_pixels_revision_tests:
    // the real compositor runs without loading system fonts or editing settings.
    app.comic_fonts = Some(Arc::new(comic_core::FontSet::new()));
    app.comic_fonts_loaded = true;
    app.comic_font_registry_loaded = true;
}

fn ready(app: &mut App, ctx: &egui::Context, input: PreparedEditInput) -> FinalCompositeKey {
    let output = execute(&input, &Arc::new(AtomicBool::new(false))).unwrap();
    install(app, ctx, input, output)
}

fn install(
    app: &mut App,
    ctx: &egui::Context,
    input: PreparedEditInput,
    output: EditOutput,
) -> FinalCompositeKey {
    app.prepare_final_demand_topologies();
    let unit = app
        .d3_final_prepare_unit_at(
            app.fullscreen_idx.unwrap(),
            crate::ui_fullscreen::FinalUnitPosition::Next,
        )
        .unwrap();
    let (permit, reservation) = app
        .final_prepare_cpu
        .try_start(
            unit.clone(),
            output_bytes(&input).unwrap(),
            app.final_prepare_wake(ctx),
        )
        .unwrap();
    drop(permit);
    let key = input.key;
    let input = Arc::new(input);
    app.final_effect_pending.insert(
        key,
        FinalEffectPending {
            pdf_metadata: input.pdf,
            canonical_dims: input.canonical,
            source_size: input.source_size,
            cancel: Arc::new(AtomicBool::new(false)),
            state: FinalEffectState::CpuReady(FinalEffectReady::Edited(PreparedEditPixels {
                input: input.clone(),
                output: Ok(output),
                _reservation: reservation,
                elapsed_ms: 0.0,
            })),
            items_generation: app.items_generation,
            output_complete: true,
            nearest_sampler: input.plan.needs_nearest_sampler(),
            origin: FinalEffectOrigin::Speculative(unit),
            gpu_diagnostics: final_prepare_budget::FinalPrepareGpuDiagnostics::requested("next", 0),
        },
    );
    key
}

fn root_frame(app: &mut App, ctx: &egui::Context, key: FinalCompositeKey, frame: u64) {
    begin_registration_frame(app, frame);
    assert!(app.adopt_prepared_edit(
        ctx,
        key,
        FinalPrepareAdoption::RootFinalized(None, crate::ui_fullscreen::FinalUnitPosition::Next),
        false
    ));
}

fn first_texture(app: &App, key: FinalCompositeKey) -> egui::TextureId {
    let FinalEffectState::Registering(registration) = &app.final_effect_pending[&key].state else {
        panic!("registration must retain its first private texture")
    };
    registration.final_texture.as_ref().unwrap().id()
}

#[test]
fn local_completion_publishes_cpu_stages_and_final_atomically() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    local(&mut app, 1, true);
    let input = app.prepared_edit_input(1).unwrap();
    let local_key = input.local_key;
    let expected = execute(&input, &Arc::new(AtomicBool::new(false))).unwrap();
    let final_pixels = expected.final_pixels.as_ref().unwrap().clone();
    let local_pixels = expected.local.as_ref().unwrap().pixels.clone();
    let key = install(&mut app, &ctx, input, expected);
    assert!(!app.local_adjust_cache.contains_key(&local_key));
    assert!(!app.edit_result_cache.contains_key(&key.edit_key));
    root_frame(&mut app, &ctx, key, 501);
    assert!(app.final_effect_pending.is_empty());
    assert!(app.local_adjust_cache[&local_key].texture.is_none());
    assert!(app.edit_result_cache[&key.edit_key].texture.is_none());
    assert!(Arc::ptr_eq(
        &app.local_adjust_cache[&local_key].pixels,
        &local_pixels
    ));
    assert!(Arc::ptr_eq(
        &app.final_composite_cache[&key].pixels,
        &final_pixels
    ));
    assert!(app.local_adjust_is_materialized(1));
    app.maybe_start_local_adjust_render(1);
    assert!(app.local_adjust_pending.is_empty());
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
}

#[test]
fn annotation_registration_is_private_for_first_frame_and_reuses_final_handle() {
    for display_takeover in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        local(&mut app, 1, true);
        annotation(&mut app, 1);
        let input = app.prepared_edit_input(1).unwrap();
        let local_key = input.local_key;
        let key = ready(&mut app, &ctx, input);
        root_frame(&mut app, &ctx, key, 510);
        let final_texture = first_texture(&app, key);
        assert!(!app.final_composite_cache.contains_key(&key));
        assert!(!app.local_adjust_cache.contains_key(&local_key));
        assert!(!app.comic_cache.contains_key(&1));
        assert!(app.final_prepare_cpu.snapshot().bytes > 0);
        if display_takeover {
            app.resolve_prepared_edit_for_display(&ctx, 1);
        } else {
            root_frame(&mut app, &ctx, key, 511);
        }
        assert!(!app.final_effect_pending.contains_key(&key));
        assert_eq!(app.final_composite_cache[&key].texture.id(), final_texture);
        assert!(Arc::ptr_eq(
            &app.final_composite_cache[&key].pixels,
            &app.comic_cache[&1].base
        ));
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    }
}

#[test]
fn completed_channel_is_received_before_display_classifies_computing() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    local(&mut app, 1, true);
    let input = app.prepared_edit_input(1).unwrap();
    let key = ready(&mut app, &ctx, input);
    let pending = app.final_effect_pending.get_mut(&key).unwrap();
    let FinalEffectState::CpuReady(FinalEffectReady::Edited(result)) = std::mem::replace(
        &mut pending.state,
        FinalEffectState::CpuReady(FinalEffectReady::Legacy(FinalCompositeResult::Cancelled)),
    ) else {
        unreachable!()
    };
    let input = result.input.clone();
    let reservation = result._reservation.clone();
    let (tx, rx) = mpsc::channel();
    tx.send(result)
        .unwrap_or_else(|_| panic!("connected completion channel"));
    pending.state = FinalEffectState::Computing(FinalEffectReceiver::Edited {
        input,
        rx,
        _reservation: reservation,
    });
    app.resolve_prepared_edit_for_display(&ctx, 1);
    assert!(app.final_effect_pending.is_empty());
    assert!(app.final_composite_cache.contains_key(&key));
    assert!(
        app.local_adjust_cache[&app.current_local_adjust_key(1)]
            .texture
            .is_none()
    );
}

#[test]
fn incomplete_computing_display_and_every_edit_state_revoke_publication() {
    for state in 0..3 {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        local(&mut app, 1, true);
        annotation(&mut app, 1);
        let input = app.prepared_edit_input(1).unwrap();
        let key = ready(&mut app, &ctx, input);
        let mut sender = None;
        if state == 0 {
            let pending = app.final_effect_pending.get_mut(&key).unwrap();
            let FinalEffectState::CpuReady(FinalEffectReady::Edited(result)) = std::mem::replace(
                &mut pending.state,
                FinalEffectState::CpuReady(FinalEffectReady::Legacy(
                    FinalCompositeResult::Cancelled,
                )),
            ) else {
                unreachable!()
            };
            let (tx, rx) = mpsc::channel();
            pending.state = FinalEffectState::Computing(FinalEffectReceiver::Edited {
                input: result.input.clone(),
                rx,
                _reservation: result._reservation.clone(),
            });
            sender = Some(tx);
        } else if state == 2 {
            root_frame(&mut app, &ctx, key, 520);
            first_texture(&app, key);
        }
        let cancel = app.final_effect_pending[&key].cancel.clone();
        if state == 0 {
            app.resolve_prepared_edit_for_display(&ctx, 1);
        } else {
            app.cancel_prepared_edit_for_entry(1);
        }
        assert!(cancel.load(Ordering::Relaxed));
        assert!(app.final_effect_pending.is_empty());
        assert!(!app.final_composite_cache.contains_key(&key));
        assert!(app.local_adjust_cache.is_empty());
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
        drop(sender);
    }
}

#[test]
fn cold_font_missing_doc_is_adopted_without_replacing_existing_final_or_respawning() {
    for empty in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let original = app.ensure_final_composite_texture(&ctx, 1).unwrap();
        let original_pixels = app
            .exact_complete_final_composite_entry(1)
            .unwrap()
            .1
            .pixels
            .clone();
        app.comic_pages.insert(1);
        let mut input = app.prepared_edit_input(1).unwrap();
        let path = app.tmp.path().join("cold-comic.db");
        let doc = if empty {
            Vec::new()
        } else {
            missing_font_doc()
        };
        let db = crate::comic_db::ComicDb::open_at(&path).unwrap();
        db.set(&input.page_key, &doc).unwrap();
        drop(db);
        input.comic.as_mut().unwrap().path = path;
        let page_key = input.page_key.clone();
        let key = ready(&mut app, &ctx, input);
        app.resolve_prepared_edit_for_display(&ctx, 1);
        assert_eq!(app.comic_docs[&page_key], doc);
        assert_eq!(app.final_composite_cache[&key].texture.id(), original.id());
        assert!(Arc::ptr_eq(
            &app.final_composite_cache[&key].pixels,
            &original_pixels
        ));
        assert!(app.comic_cache.is_empty());
        assert!(app.prepared_edit_input(1).is_none());
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    }
}

#[test]
fn missing_annotation_font_does_not_prevent_local_and_final_publication() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    local(&mut app, 1, true);
    app.comic_pages.insert(1);
    let page_key = app.page_path_key(1).unwrap();
    app.comic_docs.insert(page_key, missing_font_doc());
    let input = app.prepared_edit_input(1).unwrap();
    assert!(input.comic.is_none());
    let key = ready(&mut app, &ctx, input);
    app.resolve_prepared_edit_for_display(&ctx, 1);
    assert!(app.final_composite_cache.contains_key(&key));
    assert!(app.local_adjust_is_materialized(1));
    assert!(app.comic_cache.is_empty());
    assert!(app.prepared_edit_input(1).is_none());
}

#[test]
fn stale_document_excludes_only_annotation_and_never_overwrites_current_doc() {
    for register_first in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        local(&mut app, 1, true);
        annotation(&mut app, 1);
        let input = app.prepared_edit_input(1).unwrap();
        let page_key = input.page_key.clone();
        let key = ready(&mut app, &ctx, input);
        if register_first {
            root_frame(&mut app, &ctx, key, 530);
        }
        app.comic_docs.insert(page_key.clone(), Vec::new());
        app.resolve_prepared_edit_for_display(&ctx, 1);
        assert!(app.comic_docs[&page_key].is_empty());
        assert!(app.comic_cache.is_empty());
        assert!(app.final_composite_cache.contains_key(&key));
        assert!(app.local_adjust_is_materialized(1));
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    }
}

#[test]
fn stale_params_source_or_generation_discards_all_outputs_before_publication() {
    for change in 0..4 {
        for register_first in [false, true] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx);
            local(&mut app, 1, true);
            annotation(&mut app, 1);
            let input = app.prepared_edit_input(1).unwrap();
            let key = ready(&mut app, &ctx, input);
            if register_first {
                root_frame(&mut app, &ctx, key, 540);
            }
            match change {
                0 => {
                    app.adjustment_page_params.get_mut(&1).unwrap().brightness += 1.0;
                }
                1 => {
                    app.items_generation = app.items_generation.wrapping_add(1);
                }
                2 => {
                    *app.local_adjust_generation.entry(1).or_default() += 1;
                }
                _ => {
                    let FsCacheEntry::Static { pixels, .. } = app.fs_cache.get_mut(&1).unwrap()
                    else {
                        unreachable!()
                    };
                    *pixels = Arc::new((**pixels).clone());
                }
            }
            app.resolve_prepared_edit_for_display(&ctx, 1);
            assert!(app.final_effect_pending.is_empty());
            assert!(!app.final_composite_cache.contains_key(&key));
            assert!(app.local_adjust_cache.is_empty());
            assert!(app.comic_cache.is_empty());
            assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
        }
    }
}

#[test]
fn execute_matches_ordinary_local_then_final_and_shares_identity_allocations() {
    for active in [false, true] {
        for nearest in [false, true] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx);
            local(&mut app, 1, active);
            let params = app.adjustment_page_params.get_mut(&1).unwrap();
            params.brightness = 0.0;
            params.post_filter = if nearest {
                crate::adjustment::PostFilter::Nearest
            } else {
                crate::adjustment::PostFilter::None
            };
            if !active && !nearest {
                assert!(
                    app.prepared_edit_input(1).is_none(),
                    "normalized inactive layers require no work"
                );
                continue;
            }
            let input = app.prepared_edit_input(1).unwrap();
            let cancel = Arc::new(AtomicBool::new(false));
            let ordinary_local = crate::edit_source::materialize_local_adjust(
                &input.source,
                app.local_adjust_page_layers[&1].clone(),
                &cancel,
            )
            .unwrap();
            let FinalCompositeResult::Ready {
                pixels: ordinary, ..
            } = execute_final_composite(
                ordinary_local.pixels,
                build_final_composite_plan_without_ai(&input.params, None),
                &cancel,
            )
            else {
                panic!("ordinary final")
            };
            let output = execute(&input, &cancel).unwrap();
            assert_eq!(
                output.final_pixels.as_ref().unwrap().as_ref(),
                ordinary.as_ref()
            );
            assert!(Arc::ptr_eq(
                output.final_pixels.as_ref().unwrap(),
                output.final_upload.as_ref().unwrap()
            ));
            assert_eq!(
                Arc::ptr_eq(
                    output.edit.as_ref().unwrap(),
                    output.final_pixels.as_ref().unwrap()
                ),
                !nearest
            );
            assert_eq!(
                output_bytes(&input),
                Some(
                    image_bytes(input.source.size).unwrap()
                        * (u64::from(active) + u64::from(nearest))
                )
            );
        }
    }
}

#[test]
fn completed_erase_pixels_are_the_input_and_after_ai_does_not_reapply_adjustment() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    local(&mut app, 1, true);
    let pixels = Arc::new(egui::ColorImage::filled([12, 18], egui::Color32::GOLD));
    let texture = ctx.load_texture(
        "completed-erase",
        pixels.clone(),
        DISPLAY_IMAGE_TEXTURE_OPTIONS,
    );
    app.mask_pages.insert(1);
    let erase_key = app.current_erase_result_key(1);
    app.erase_result_cache.insert(
        erase_key,
        EraseResultCacheEntry::raster(pixels.clone(), texture),
    );
    let input = app.prepared_edit_input(1).unwrap();
    assert!(matches!(input.stage, InputStage::Erase));
    assert!(Arc::ptr_eq(&input.source, &pixels));
    let output = execute(&input, &Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(
        output.local.as_ref().unwrap().pixels.as_ref(),
        pixels.as_ref()
    );

    for used_upscale in [false, true] {
        let params = app.adjustment_page_params.get_mut(&1).unwrap();
        params.upscale_model = Some("realesr_general_v3".to_owned());
        let edit_key = app.current_edit_result_key(1);
        let key = app
            .final_ai_key_for_pixels(edit_key, pixels.size, app.effective_params(1))
            .unwrap();
        app.final_ai_cache.insert(
            key,
            FinalAiEntry {
                canonical_dims: None,
                source_size: pixels.size,
                pixels: pixels.clone(),
                used_upscale,
            },
        );
        let input = app.prepared_edit_input(1).unwrap();
        assert!(matches!(input.final_input, PreparedFinalInput::AfterAi(_)));
        let cancel = Arc::new(AtomicBool::new(false));
        let FinalCompositeResult::Ready {
            pixels: expected, ..
        } = execute_final_composite(
            pixels.clone(),
            build_final_composite_plan_after_ai(app.effective_params(1), None, used_upscale),
            &cancel,
        )
        else {
            panic!("ordinary after-AI final")
        };
        let output = execute(&input, &cancel).unwrap();
        assert!(
            output.local.is_some(),
            "missing lower CPU stage is completed independently of AI"
        );
        assert!(output.edit.is_some());
        assert_eq!(
            output.final_pixels.as_ref().unwrap().as_ref(),
            expected.as_ref()
        );
    }
}

#[test]
fn byte_accounting_is_checked_and_shared_unclamped_outputs_are_not_reserved_twice() {
    assert_eq!(image_bytes([usize::MAX, usize::MAX]), None);
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    local(&mut app, 1, true);
    annotation(&mut app, 1);
    app.adjustment_page_params.get_mut(&1).unwrap().brightness = 0.0;
    let input = app.prepared_edit_input(1).unwrap();
    assert_eq!(
        output_bytes(&input),
        Some(image_bytes(input.source.size).unwrap() * 2)
    );
}

#[test]
fn local_conceal_final_and_comic_use_the_same_cumulative_cpu_chain() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    local(&mut app, 1, true);
    annotation(&mut app, 1);
    app.conceal_pages.insert(1);
    let mut input = app.prepared_edit_input(1).unwrap();
    let path = app.tmp.path().join("conceal-input.db");
    let db = crate::conceal_db::ConcealDb::open_at(&path).unwrap();
    let [w, h] = input.source.size;
    let mut mask = vec![false; w * h];
    mask[0] = true;
    db.set(&input.page_key, &mask, &[], w, h).unwrap();
    drop(db);
    input.conceal.as_mut().unwrap().0 = path;
    let output = execute(&input, &Arc::new(AtomicBool::new(false))).unwrap();
    assert!(Arc::ptr_eq(
        output.edit.as_ref().unwrap(),
        output.conceal.as_ref().unwrap()
    ));
    let cancel = Arc::new(AtomicBool::new(false));
    let FinalCompositeResult::Ready {
        pixels: ordinary_final,
        ..
    } = execute_final_composite(
        output.conceal.as_ref().unwrap().clone(),
        build_final_composite_plan_without_ai(&input.params, None),
        &cancel,
    )
    else {
        panic!("ordinary final")
    };
    assert_eq!(
        output.final_pixels.as_ref().unwrap().as_ref(),
        ordinary_final.as_ref()
    );
    let comic = input.comic.as_ref().unwrap();
    let scale = ordinary_final.size[0].max(ordinary_final.size[1]) as f32
        / comic.canonical_size[0].max(comic.canonical_size[1]) as f32;
    let doc = comic_core::scale_scene(comic.doc.as_ref().unwrap(), scale);
    let layers = comic_core::bake_annotation_layers(
        &doc,
        w,
        h,
        comic.fonts.as_ref().unwrap(),
        &comic_core::StampImages::new(),
    );
    let expected = crate::comic_overlay::composite_annotation_layers(&ordinary_final, &layers);
    assert_eq!(output.comic.as_ref().unwrap().pixels.as_ref(), &expected);
    let key = install(&mut app, &ctx, input, output);
    app.resolve_prepared_edit_for_display(&ctx, 1);
    assert!(app.conceal_cache[&1].texture.is_none());
    assert!(
        app.local_adjust_cache[&app.current_local_adjust_key(1)]
            .texture
            .is_none()
    );
    assert!(Arc::ptr_eq(
        &app.final_composite_cache[&key].pixels,
        &app.comic_cache[&1].base
    ));
}

#[test]
fn font_requirements_include_name_plate_and_recheck_when_loaded_keys_change() {
    let fonts = Some(Arc::new(comic_core::FontSet::new()));
    let mut window = comic_core::MessageWindowObject::default();
    window.name_plate.name.font_key = "plate".to_owned();
    let doc = vec![comic_core::AnnotationObject::new_message_window(
        1,
        (0.0, 0.0),
        window,
    )];
    let mut keys = std::collections::HashSet::new();
    assert!(!fonts_satisfy(&doc, &fonts, &keys));
    keys.insert("plate".to_owned());
    assert!(fonts_satisfy(&doc, &fonts, &keys));
    assert!(!fonts_satisfy(&doc, &None, &keys));

    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    app.ensure_final_composite_texture(&ctx, 1).unwrap();
    app.comic_pages.insert(1);
    app.comic_fonts = fonts;
    let page_key = app.page_path_key(1).unwrap();
    app.comic_docs.insert(page_key, doc);
    assert!(app.prepared_edit_input(1).is_none());
    app.comic_loaded_font_keys = keys;
    assert!(app.prepared_edit_input(1).is_some());
}

#[test]
fn lut_and_segmentation_conflicts_use_page_identity_instead_of_stale_indices() {
    for segmentation in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        local(&mut app, 1, true);
        let owner = app.capture_page_edit_owner(2).unwrap();
        let document = app.local_adjust_page_layers[&1].clone();
        if segmentation {
            let (_tx, rx) = mpsc::channel();
            app.local_adjust_segmentation_pending = Some(LocalAdjustSegmentationPending {
                owner,
                fs_idx: 1,
                layer_idx: 0,
                document,
                cancel: Arc::new(AtomicBool::new(false)),
                rx,
            });
        } else {
            let (_tx, rx) = mpsc::channel();
            app.local_adjust_lut_pending = Some(LocalAdjustLutLoadPending {
                owner,
                fs_idx: 1,
                layer_idx: 0,
                document,
                path: PathBuf::from("unused.cube"),
                rx,
            });
        }
        assert!(
            app.prepared_edit_input(1).is_some(),
            "another page's stale idx cannot block B"
        );
        let key = app.page_path_key(1).unwrap();
        if segmentation {
            app.local_adjust_segmentation_pending
                .as_mut()
                .unwrap()
                .owner
                .target
                .page_key = key;
        } else {
            app.local_adjust_lut_pending
                .as_mut()
                .unwrap()
                .owner
                .target
                .page_key = key;
        }
        assert!(app.prepared_edit_input(1).is_none());
    }
}

#[test]
fn cold_local_result_cannot_replace_a_document_hydrated_before_publication() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    local(&mut app, 1, true);
    let layers = app.local_adjust_page_layers.remove(&1).unwrap();
    let mut input = app.prepared_edit_input(1).unwrap();
    let path = app.tmp.path().join("cold-local.db");
    let db = crate::local_adjust_db::LocalAdjustDb::open_at(&path).unwrap();
    db.set_layers(&input.page_key, &layers).unwrap();
    drop(db);
    input.local = Some(LocalMaterializeLayers::Database {
        path,
        page_key: input.page_key.clone(),
    });
    let key = ready(&mut app, &ctx, input);
    local(&mut app, 1, false);
    let hydrated = app.local_adjust_page_layers[&1].clone();
    app.resolve_prepared_edit_for_display(&ctx, 1);
    assert_eq!(app.local_adjust_page_layers[&1], hydrated);
    assert!(!app.final_composite_cache.contains_key(&key));
    assert!(app.local_adjust_cache.is_empty());
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
}

#[test]
fn overbudget_retains_the_result_and_private_handle_without_partial_publication() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    local(&mut app, 1, true);
    annotation(&mut app, 1);
    let input = app.prepared_edit_input(1).unwrap();
    let key = ready(&mut app, &ctx, input);
    root_frame(&mut app, &ctx, key, 560);
    let handle = first_texture(&app, key);
    let bytes = app.final_prepare_cpu.snapshot().bytes;
    for _ in 0..3 {
        app.adopt_prepared_edit(
            &ctx,
            key,
            FinalPrepareAdoption::RootFinalized(
                Some(FinalPrepareGpuDeferred::OverBudget),
                crate::ui_fullscreen::FinalUnitPosition::Next,
            ),
            false,
        );
        assert_eq!(first_texture(&app, key), handle);
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, bytes);
        assert!(app.final_composite_cache.is_empty());
        assert!(app.comic_cache.is_empty());
    }
    app.cancel_prepared_edit_for_entry(1);
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
}

#[cfg(windows)]
fn seed_context(owner: &mut App, ctx: &egui::Context) {
    owner.items = (0..4)
        .map(|idx| GridItem::Image(PathBuf::from(format!("C:/b-owner/{idx}.png"))))
        .collect();
    owner.visible_indices = vec![0, 1, 2, 3];
    owner.details_order = owner.visible_indices.clone();
    owner.thumbnails = vec![ThumbnailState::Pending; 4];
    owner.image_metas = vec![None; 4];
    owner.fullscreen_idx = Some(0);
    owner.spread_mode = crate::settings::SpreadMode::Single;
    owner.reading_flow = crate::settings::ReadingFlow::Paged;
    owner.settings.page_turn_animation_enabled = true;
    for idx in 0..4 {
        let pixels = Arc::new(egui::ColorImage::filled(
            [80, 120],
            egui::Color32::LIGHT_BLUE,
        ));
        let tex = ctx.load_texture(
            format!("b-owner-{idx}"),
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
    local(owner, 1, true);
    annotation(owner, 1);
}

#[cfg(windows)]
#[test]
fn ready_and_registering_owners_survive_park_and_resume_without_sibling_mutation() {
    for registering in [false, true] {
        let ctx = egui::Context::default();
        crate::ui_fullscreen::page_turn_submission_tests::install_presentation(&ctx);
        let mut app = app(&ctx);
        let window = 96_571;
        let id = app.build_active_context_for_test(Some(window), DetachedSource::Image, |owner| {
            seed_context(owner, &ctx);
            owner.viewer_presentation = ViewerPresentation::DetachedWindow;
            owner.detached_viewer_independent_active = true;
            owner.set_detached_window_binding_for_test(Some(window));
        });
        let (key, cancel, handle) = app
            .with_active_viewer_context(|owner| {
                owner.activate_mounted_as_independent_detached(window);
                let input = owner.prepared_edit_input(1).unwrap();
                let key = ready(owner, &ctx, input);
                let handle = if registering {
                    root_frame(owner, &ctx, key, 570);
                    Some(first_texture(owner, key))
                } else {
                    None
                };
                (key, owner.final_effect_pending[&key].cancel.clone(), handle)
            })
            .unwrap();
        let bytes = app.final_prepare_cpu.snapshot().bytes;
        assert!(app.pause_current_active_viewer_context(&ctx));
        app.bump_input_generation(1);
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, bytes);
        app.with_viewer_context(id, |owner| {
            assert!(Arc::ptr_eq(
                &owner.final_effect_pending[&key].cancel,
                &cancel
            ));
            assert!(owner.final_composite_cache.is_empty());
            if let Some(handle) = handle {
                assert_eq!(first_texture(owner, key), handle);
            }
        })
        .unwrap();
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(app.activate_detached_image_window_snapshot(&ctx, window));
        app.with_active_viewer_context(|owner| {
            owner.resolve_prepared_edit_for_display(&ctx, 1);
            assert!(owner.final_effect_pending.is_empty());
            assert!(owner.final_composite_cache.contains_key(&key));
            if let Some(handle) = handle {
                assert_eq!(owner.final_composite_cache[&key].texture.id(), handle);
            }
        })
        .unwrap();
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
        assert!(app.final_effect_pending.is_empty());
        assert!(!app.final_composite_cache.contains_key(&key));
    }
}

#[cfg(windows)]
#[test]
fn fork_has_no_prepared_edit_owner_or_cancel_authority() {
    for registering in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        local(&mut app, 1, true);
        annotation(&mut app, 1);
        let input = app.prepared_edit_input(1).unwrap();
        let key = ready(&mut app, &ctx, input);
        if registering {
            root_frame(&mut app, &ctx, key, 580);
        }
        let cancel = app.final_effect_pending[&key].cancel.clone();
        let bytes = app.final_prepare_cpu.snapshot().bytes;
        let path = app.tmp.path().join("prepared-fork");
        let fork = app.fork_materialized_still_context(&path, 0);
        app.with_viewer_context(fork, |owner| {
            assert!(owner.final_effect_pending.is_empty());
            assert!(owner.final_composite_cache.is_empty());
            owner.cancel_prepared_edit_for_entry(1);
        })
        .unwrap();
        app.retire_context(fork, "prepared-edit-test", |_| ())
            .unwrap();
        assert!(Arc::ptr_eq(&app.final_effect_pending[&key].cancel, &cancel));
        assert!(!cancel.load(Ordering::Relaxed));
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, bytes);
        app.cancel_prepared_edit_for_entry(1);
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    }
}

#[test]
fn normalized_cpu_stages_repair_missing_identity_final_without_new_image_allocations() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    app.adjustment_page_params.get_mut(&1).unwrap().brightness = 0.0;
    let (old, _) = app.seed_prepared_edit_entry_test(&ctx, 1, true);
    let local_pixels = app.current_local_adjust_pixels(1).unwrap();
    app.final_composite_cache
        .remove_with_drop_reason(&old, "test eviction");
    let input = app
        .prepared_edit_input(1)
        .expect("CPU-only pixels still need a display allocation");
    assert!(input.local.is_none());
    assert_eq!(output_bytes(&input), Some(0));
    let key = ready(&mut app, &ctx, input);
    root_frame(&mut app, &ctx, key, 1001);
    assert!(Arc::ptr_eq(
        &app.final_composite_cache[&key].pixels,
        &local_pixels
    ));
    app.maybe_start_local_adjust_render(1);
    assert!(app.local_adjust_pending.is_empty());
}

#[test]
fn resident_final_does_not_hide_missing_or_unnormalized_local_cpu_stages() {
    for disabled in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let (key, _) = app.seed_prepared_edit_entry_test(&ctx, 1, true);
        let final_pixels = app.final_composite_cache[&key].pixels.clone();
        let final_texture = app.final_composite_cache[&key].texture.id();
        let mut layer = local_adjust_core::LocalAdjustmentLayer::new(
            "resize",
            local_adjust_core::LocalMask::Raster(local_adjust_core::RasterMask::empty(2, 3)),
            local_adjust_core::LocalEffect::None,
        );
        layer.enabled = !disabled;
        app.local_adjust_page_layers
            .insert(1, Arc::new(vec![layer]));
        assert!(!app.local_adjust_is_materialized(1));
        let input = app.prepared_edit_input(1).unwrap();
        assert!(input.local.is_some());
        assert!(matches!(
            input.final_input,
            PreparedFinalInput::Completed { .. }
        ));
        let ordinary = crate::edit_source::materialize_local_adjust(
            &input.source,
            app.local_adjust_page_layers[&1].clone(),
            &Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let key = ready(&mut app, &ctx, input);
        root_frame(&mut app, &ctx, key, 1005);
        assert_eq!(app.local_adjust_page_layers[&1], ordinary.layers);
        assert!(app.local_adjust_is_materialized(1));
        assert_eq!(app.final_composite_cache[&key].texture.id(), final_texture);
        assert!(Arc::ptr_eq(
            &app.final_composite_cache[&key].pixels,
            &final_pixels
        ));
        app.maybe_start_local_adjust_render(1);
        assert!(app.local_adjust_pending.is_empty());
        assert!(
            app.final_composite_cache.contains_key(&key),
            "no later same-key invalidation"
        );
        assert!(
            matches!(
                app.admit_final_prepare_registration([1, 1], DISPLAY_IMAGE_TEXTURE_OPTIONS),
                FinalPrepareGpuAdmission::Admitted
            ),
            "shared handle did not consume the ROOT slot"
        );
    }
}

#[test]
fn actual_entry_and_original_resume_handlers_revoke_b_before_existing_owners() {
    for entry in [
        "local", "erase", "conceal", "text", "crop", "sns", "original", "resume",
    ] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let (key, cancel) = app.seed_prepared_edit_entry_test(&ctx, 1, false);
        app.fullscreen_idx = Some(1);
        match entry {
            "local" => app.enter_local_adjust_mode(&ctx),
            "erase" => app.enter_erase_mode(1),
            "conceal" => app.enter_conceal_mode(&ctx, 1),
            "text" => app.enter_text_mode(1),
            "crop" => app.enter_export_crop_mode(1),
            "sns" => {
                let _ = app.enter_sns_split_mode(1);
            }
            "original" => {
                app.ensure_original_material_for_entry(
                    1,
                    OriginalMaterialEntry::LocalAdjustment {
                        requested_fs_idx: 1,
                    },
                );
            }
            _ => app.resume_original_material_entry(
                &ctx,
                1,
                OriginalMaterialEntry::LocalAdjustment {
                    requested_fs_idx: 1,
                },
            ),
        }
        assert!(cancel.load(Ordering::Relaxed), "{entry}");
        assert!(!app.final_effect_pending.contains_key(&key), "{entry}");
        assert!(!app.final_composite_cache.contains_key(&key), "{entry}");
        app.cancel_local_adjust_pending_for_idx(1);
    }
}

#[test]
fn actual_final_and_comic_display_entries_receive_before_starting_ordinary_producers() {
    for comic in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        local(&mut app, 1, true);
        annotation(&mut app, 1);
        let input = app.prepared_edit_input(1).unwrap();
        let key = ready(&mut app, &ctx, input);
        if comic {
            assert!(app.ensure_comic_composite_texture(&ctx, 1).is_some());
        } else {
            assert!(app.ensure_final_composite_texture(&ctx, 1).is_some());
        }
        assert!(app.final_effect_pending.is_empty());
        assert!(app.final_composite_cache.contains_key(&key));
        assert!(app.local_adjust_pending.is_empty());
        assert!(app.comic_bake_pending.is_empty());
    }
}

#[test]
fn old_next_b_moved_to_previous_never_spends_the_next_sweep_registration_slot() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    local(&mut app, 1, true);
    annotation(&mut app, 1);
    let input = app.prepared_edit_input(1).unwrap();
    let key = ready(&mut app, &ctx, input);
    app.fullscreen_idx = Some(2);
    app.prepare_final_demand_topologies();
    assert_eq!(
        app.final_prepare_key_position(key),
        Some(crate::ui_fullscreen::FinalUnitPosition::Previous)
    );
    root_frame(&mut app, &ctx, key, 1002);
    assert!(matches!(
        app.final_effect_pending[&key].state,
        FinalEffectState::CpuReady(_)
    ));
    app.adopt_prepared_edit(
        &ctx,
        key,
        FinalPrepareAdoption::RootFinalized(
            None,
            crate::ui_fullscreen::FinalUnitPosition::Previous,
        ),
        false,
    );
    first_texture(&app, key);
    app.cancel_prepared_edit_for_entry(1);
}

#[test]
fn cancelled_b_worker_keeps_both_running_leases_until_it_actually_exits() {
    use crate::app::final_prepare::tests::{WorkerHold, wait_idle};
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    local(&mut app, 1, true);
    annotation(&mut app, 1);
    app.prepare_final_demand_topologies();
    let unit = app
        .d3_final_prepare_unit_at(0, crate::ui_fullscreen::FinalUnitPosition::Next)
        .unwrap();
    let hold = WorkerHold::new();
    assert!(app.spawn_prepared_edit_job(&ctx, 1, unit.clone(), &mut false));
    hold.entered();
    assert_eq!(app.final_prepare_cpu.snapshot().running, 1);
    assert_eq!(app.comic_bake_inflight.load(Ordering::Relaxed), 1);
    app.fullscreen_idx = Some(2);
    app.prepare_final_demand_topologies();
    app.cancel_previous_computing_for_next();
    assert!(app.final_effect_pending.is_empty());
    assert_eq!(app.final_prepare_cpu.snapshot().running, 1);
    assert_eq!(app.comic_bake_inflight.load(Ordering::Relaxed), 1);
    assert!(
        app.final_prepare_cpu
            .try_start(unit, 1, app.final_prepare_wake(&ctx))
            .is_err()
    );
    hold.release();
    wait_idle(&app);
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    assert_eq!(app.comic_bake_inflight.load(Ordering::Relaxed), 0);
}

#[test]
fn final_and_annotation_use_independent_triangle_clamps_after_full_size_canonical_bake() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    local(&mut app, 1, true);
    annotation(&mut app, 1);
    let mut input = app.prepared_edit_input(1).unwrap();
    input.source = Arc::new(egui::ColorImage::filled(
        [100_000, 8],
        egui::Color32::from_rgb(100, 150, 200),
    ));
    input.comic.as_mut().unwrap().canonical_size = [1_000, 8];
    let output = execute(&input, &Arc::new(AtomicBool::new(false))).unwrap();
    let final_pixels = output.final_pixels.as_ref().unwrap();
    let comic = output.comic.as_ref().unwrap();
    assert_eq!(final_pixels.size, [100_000, 8]);
    assert!(!Arc::ptr_eq(
        final_pixels,
        output.final_upload.as_ref().unwrap()
    ));
    assert!(!Arc::ptr_eq(&comic.pixels, &comic.upload));
    assert_eq!(
        output.final_upload.as_ref().unwrap().as_ref(),
        &clamp_for_gpu(final_pixels).into_owned()
    );
    let mut stamps = std::collections::HashMap::new();
    let expected = crate::edit_source::comic_composite(
        final_pixels,
        input.comic.as_ref().unwrap().doc.as_ref().unwrap(),
        [1_000, 8],
        input.comic.as_ref().unwrap().fonts.as_ref().unwrap(),
        &mut stamps,
        &AtomicBool::new(false),
    );
    assert_eq!(comic.pixels.as_ref(), expected.as_ref());
    assert_eq!(
        comic.upload.as_ref(),
        &clamp_for_gpu(&expected).into_owned()
    );
    assert_eq!(
        output_bytes(&input),
        Some(
            image_bytes([100_000, 8]).unwrap() * 3
                + image_bytes(output.final_upload.as_ref().unwrap().size).unwrap() * 2
        )
    );
}

#[test]
fn old_computing_and_new_ready_page_requests_are_cancelled_by_their_own_keys() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    let (old, old_cancel) = app.seed_prepared_edit_entry_test(&ctx, 1, false);
    let pending = app.final_effect_pending.get_mut(&old).unwrap();
    let FinalEffectState::CpuReady(FinalEffectReady::Edited(result)) = std::mem::replace(
        &mut pending.state,
        FinalEffectState::CpuReady(FinalEffectReady::Legacy(FinalCompositeResult::Cancelled)),
    ) else {
        unreachable!()
    };
    let (sender, rx) = mpsc::channel();
    pending.state = FinalEffectState::Computing(FinalEffectReceiver::Edited {
        input: result.input.clone(),
        rx,
        _reservation: result._reservation.clone(),
    });
    drop(result);
    // Model an accepted sibling edit publication without touching its old
    // receiver. The new generation has completed before the display pass.
    app.local_adjust_generation.insert(1, 1);
    let input = app.prepared_edit_input(1).unwrap();
    let new = ready(&mut app, &ctx, input);
    assert_eq!(app.final_effect_pending.len(), 2);
    app.resolve_prepared_edit_for_display(&ctx, 1);
    assert!(old_cancel.load(Ordering::Relaxed));
    assert!(app.final_effect_pending.is_empty());
    assert!(!app.final_composite_cache.contains_key(&old));
    assert!(app.final_composite_cache.contains_key(&new));
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    drop(sender);
}

#[test]
fn previous_pressure_reclaims_b_ready_and_registering_without_touching_current_next() {
    for registering in [false, true] {
        for previous in [false, true] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx);
            local(&mut app, 1, true);
            annotation(&mut app, 1);
            let input = app.prepared_edit_input(1).unwrap();
            let key = ready(&mut app, &ctx, input);
            if registering {
                root_frame(&mut app, &ctx, key, 1006);
            }
            let unit = app.final_effect_pending[&key]
                .origin
                .unit()
                .unwrap()
                .clone();
            let cancel = app.final_effect_pending[&key].cancel.clone();
            if previous {
                app.fullscreen_idx = Some(2);
                app.prepare_final_demand_topologies();
            }
            let requester = app
                .d3_final_prepare_unit_at(
                    app.fullscreen_idx.unwrap(),
                    crate::ui_fullscreen::FinalUnitPosition::Next,
                )
                .unwrap();
            let bytes = app.final_prepare_cpu.snapshot().bytes;
            app.reclaim_previous_final_results(&[unit], &requester);
            assert_eq!(app.final_effect_pending.contains_key(&key), !previous);
            assert_eq!(cancel.load(Ordering::Relaxed), previous);
            assert_eq!(
                app.final_prepare_cpu.snapshot().bytes,
                if previous { 0 } else { bytes }
            );
        }
    }
}

#[test]
fn actual_b_perf_events_share_legacy_request_ids_and_record_takeover_and_cancel_completion() {
    use crate::app::final_prepare::tests::{WorkerHold, start, wait_idle, wait_ready};
    let directory = tempfile::tempdir().unwrap();
    let writer =
        crate::log_writer::test_support::writer(directory.path(), 10_000, 16 * 1024 * 1024);
    crate::log_writer::test_support::install_global(&writer);
    let ctx = egui::Context::default();
    let (b_id, legacy_id) = {
        let mut app = app(&ctx);
        local(&mut app, 1, true);
        annotation(&mut app, 1);
        app.prepare_final_demand_topologies();
        let unit = app
            .d3_final_prepare_unit_at(0, crate::ui_fullscreen::FinalUnitPosition::Next)
            .unwrap();
        assert!(app.spawn_prepared_edit_job(&ctx, 1, unit, &mut false));
        let key = wait_ready(&mut app, 1);
        let b_id = app.final_effect_pending[&key].gpu_diagnostics.request_id();
        root_frame(&mut app, &ctx, key, 1007);
        app.resolve_prepared_edit_for_display(&ctx, 1);
        app.frame_counter += 1;
        assert!(app.ensure_local_adjust_material(&ctx, 1).is_some());
        assert!(start(&mut app, &ctx, 2));
        let legacy_key = wait_ready(&mut app, 2);
        let legacy_id = app.final_effect_pending[&legacy_key]
            .gpu_diagnostics
            .request_id();
        app.cancel_final_effects_for_idx(2);
        (b_id, legacy_id)
    };
    let cancelled_id = {
        let mut app = app(&ctx);
        local(&mut app, 1, true);
        annotation(&mut app, 1);
        app.prepare_final_demand_topologies();
        let unit = app
            .d3_final_prepare_unit_at(0, crate::ui_fullscreen::FinalUnitPosition::Next)
            .unwrap();
        let hold = WorkerHold::new();
        assert!(app.spawn_prepared_edit_job(&ctx, 1, unit, &mut false));
        hold.entered();
        let id = app
            .final_effect_pending
            .values()
            .next()
            .unwrap()
            .gpu_diagnostics
            .request_id();
        app.fullscreen_idx = Some(2);
        app.prepare_final_demand_topologies();
        app.cancel_previous_computing_for_next();
        hold.release();
        wait_idle(&app);
        id
    };
    assert!(b_id > 0 && legacy_id > b_id && cancelled_id > legacy_id);
    // Permit return precedes the final trace just as in the legacy worker.
    // A checkpoint observes that asynchronous tail; no wall-time expectation.
    use std::io::Read;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let events: Vec<serde_json::Value> = loop {
        let snapshot = writer.snapshot().unwrap().perf.unwrap();
        let mut text = String::new();
        snapshot
            .file
            .take(snapshot.len)
            .read_to_string(&mut text)
            .unwrap();
        let rows: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        if [b_id, legacy_id, cancelled_id].iter().all(|id| {
            rows.iter().any(|row| {
                row["kind"] == "final_prepare_worker_finished" && row["request_id"] == *id
            })
        }) {
            break rows;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "worker traces did not reach their log checkpoint"
        );
        std::thread::yield_now();
    };
    assert!(writer.shutdown());
    for kind in [
        "final_prepare_cpu_started",
        "final_prepare_cpu_ready",
        "final_prepare_worker_finished",
    ] {
        assert!(
            events
                .iter()
                .any(|row| row["kind"] == kind && row["request_id"] == b_id),
            "{kind}"
        );
    }
    let registrations: Vec<_> = events
        .iter()
        .filter(|row| row["kind"] == "final_prepare_registered" && row["request_id"] == b_id)
        .collect();
    assert_eq!(registrations.len(), 2);
    assert!(
        registrations
            .iter()
            .any(|row| row["stage"] == "final" && row["display_takeover"] == false)
    );
    assert!(
        registrations
            .iter()
            .any(|row| row["stage"] == "comic" && row["display_takeover"] == true)
    );
    assert!(
        registrations
            .iter()
            .all(|row| row["load_texture_ms"].as_f64().unwrap() >= 0.0
                && row["new_bytes"].as_u64().unwrap() > 0)
    );
    assert!(
        events
            .iter()
            .any(|row| row["kind"] == "final_prepare_cpu_pressure"
                && row["reason"] == "cancelled_for_next"
                && row["request_id"] == cancelled_id)
    );
    assert!(
        events
            .iter()
            .any(|row| row["kind"] == "final_prepare_worker_finished"
                && row["cancelled"] == true
                && row["request_id"] == cancelled_id)
    );
    assert!(events.iter().any(|row| row["cat"] == "edit_materialize"
        && row["kind"] == "upload"
        && row["stage"] == "local"));
}

#[test]
fn first_private_texture_is_in_the_actual_vram_inventory_before_publication() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    local(&mut app, 1, true);
    annotation(&mut app, 1);
    let input = app.prepared_edit_input(1).unwrap();
    let size = input.source.size;
    let key = ready(&mut app, &ctx, input);
    let before = app.vram_accounting_snapshot().total_bytes();
    root_frame(&mut app, &ctx, key, 1003);
    assert!(app.final_composite_cache.is_empty());
    let increment = crate::vram_budget::rgba8_texels_to_bytes(
        crate::vram_budget::texture_size_texels(size, true),
    );
    assert_eq!(
        app.vram_accounting_snapshot().total_bytes(),
        before + increment
    );
    app.cancel_prepared_edit_for_entry(1);
    assert_eq!(app.vram_accounting_snapshot().total_bytes(), before);
}

#[test]
fn cpu_only_stage_basis_is_captured_from_its_accepted_owner_and_kept_by_edit_publication() {
    for conceal in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let (old, _) = app.seed_prepared_edit_entry_test(&ctx, 1, true);
        app.final_composite_cache
            .remove_with_drop_reason(&old, "test material selection");
        app.edit_result_cache.remove(&old.edit_key);
        let local_key = app.current_local_adjust_key(1);
        let pixels = app.local_adjust_cache[&local_key].pixels.clone();
        let basis = crate::material_display_basis::MaterialDisplayBasis::from_sampler(
            [31, 47],
            crate::source_coverage::SourceCoverage::FullSource,
        );
        app.local_adjust_cache
            .get_mut(&local_key)
            .unwrap()
            .display_basis = basis;
        if conceal {
            app.conceal_pages.insert(1);
            let generation = app.conceal_generation;
            app.conceal_cache.insert(
                1,
                ConcealCacheEntry {
                    pixels,
                    texture: None,
                    texture_options: DISPLAY_IMAGE_TEXTURE_OPTIONS,
                    display_basis: basis,
                    generation,
                },
            );
        }
        let input = app.prepared_edit_input(1).unwrap();
        assert_eq!(input.basis, basis);
        assert!(input.local.is_none() && input.conceal.is_none());
        let key = ready(&mut app, &ctx, input);
        app.resolve_prepared_edit_for_display(&ctx, 1);
        assert_eq!(app.edit_result_cache[&key.edit_key].display_basis, basis);
        assert_eq!(app.local_adjust_cache[&local_key].display_basis, basis);
        if conceal {
            assert_eq!(app.conceal_cache[&1].display_basis, basis);
        }
    }
}

#[cfg(windows)]
#[test]
fn parked_registering_texture_is_accounted_from_the_at_rest_context_without_mounting_it() {
    let ctx = egui::Context::default();
    crate::ui_fullscreen::page_turn_submission_tests::install_presentation(&ctx);
    let mut app = app(&ctx);
    let window = 96_573;
    let id = app.build_active_context_for_test(Some(window), DetachedSource::Image, |owner| {
        seed_context(owner, &ctx);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        owner.detached_viewer_independent_active = true;
        owner.set_detached_window_binding_for_test(Some(window));
    });
    let increment = app
        .with_active_viewer_context(|owner| {
            owner.activate_mounted_as_independent_detached(window);
            let input = owner.prepared_edit_input(1).unwrap();
            let key = ready(owner, &ctx, input);
            root_frame(owner, &ctx, key, 1004);
            let FinalEffectState::Registering(registration) =
                &owner.final_effect_pending[&key].state
            else {
                unreachable!()
            };
            let texture = registration.final_texture.as_ref().unwrap();
            crate::vram_budget::rgba8_texels_to_bytes(crate::vram_budget::texture_size_texels(
                texture.size(),
                true,
            ))
        })
        .unwrap();
    assert!(app.pause_current_active_viewer_context(&ctx));
    let total = app.vram_accounting_snapshot().total_bytes();
    let sibling_key = app.current_edit_result_key(1);
    app.with_viewer_context(id, |owner| owner.cancel_prepared_edit_for_entry(1))
        .unwrap();
    assert_eq!(
        app.vram_accounting_snapshot().total_bytes() + increment,
        total
    );
    assert_eq!(app.current_edit_result_key(1), sibling_key);
    assert!(app.final_effect_pending.is_empty());
    app.retire_context(id, "prepared-private-vram", |_| ())
        .unwrap();
}

#[cfg(windows)]
#[test]
fn started_b_worker_finishes_its_annotation_inside_park_without_starting_a_new_job() {
    use crate::app::final_prepare::tests::{WorkerHold, wait_idle, wait_ready};
    let ctx = egui::Context::default();
    crate::ui_fullscreen::page_turn_submission_tests::install_presentation(&ctx);
    let mut app = app(&ctx);
    let window = 96_572;
    let id = app.build_active_context_for_test(Some(window), DetachedSource::Image, |owner| {
        seed_context(owner, &ctx);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        owner.detached_viewer_independent_active = true;
        owner.set_detached_window_binding_for_test(Some(window));
    });
    let hold = WorkerHold::new();
    let (key, cancel, inflight) = app
        .with_active_viewer_context(|owner| {
            owner.activate_mounted_as_independent_detached(window);
            owner.prepare_final_demand_topologies();
            let unit = owner
                .d3_final_prepare_unit_at(0, crate::ui_fullscreen::FinalUnitPosition::Next)
                .unwrap();
            assert!(owner.spawn_prepared_edit_job(&ctx, 1, unit, &mut false));
            let key = *owner.final_effect_pending.keys().next().unwrap();
            (
                key,
                owner.final_effect_pending[&key].cancel.clone(),
                owner.comic_bake_inflight.clone(),
            )
        })
        .unwrap();
    hold.entered();
    assert!(app.pause_current_active_viewer_context(&ctx));
    assert!(!cancel.load(Ordering::Relaxed));
    assert_eq!(inflight.load(Ordering::Relaxed), 1);
    hold.release();
    wait_idle(&app);
    assert_eq!(inflight.load(Ordering::Relaxed), 0);
    app.with_viewer_context(id, |owner| {
        assert_eq!(wait_ready(owner, 1), key);
        let FinalEffectState::CpuReady(FinalEffectReady::Edited(ready)) =
            &owner.final_effect_pending[&key].state
        else {
            panic!("ready worker")
        };
        assert!(
            ready.output.as_ref().unwrap().comic.is_some(),
            "started pipeline includes annotation after park"
        );
        assert!(owner.comic_cache.is_empty());
    })
    .unwrap();
    app.retire_context(id, "prepared-worker-park", |_| ())
        .unwrap();
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    assert!(app.final_effect_pending.is_empty());
}
