//! Independent ordinary-pipeline oracles for the stage-B publication contract.
use super::*;
use crate::app::final_prepare::tests::app;
use crate::edit_source::{
    ConcealMaterialize, EditLayer, EditSourceRequest, EditSourceResult, LocalAdjustMaterialize,
    MaskSnapshot,
};

fn patterned_source(app: &mut App, ctx: &egui::Context) -> Arc<egui::ColorImage> {
    let size = app.current_raw_source_pixels(1).unwrap().size;
    let mut image = egui::ColorImage::filled(size, egui::Color32::BLACK);
    for y in 0..size[1] {
        for x in 0..size[0] {
            image.pixels[y * size[0] + x] = egui::Color32::from_rgb(
                (25 + (x * 7 + y * 3) % 180) as u8,
                (35 + (x * 2 + y * 11) % 170) as u8,
                (45 + (x * 13 + y * 5) % 160) as u8,
            );
        }
    }
    let pixels = Arc::new(image);
    let texture = ctx.load_texture(
        "review-pattern",
        pixels.clone(),
        DISPLAY_IMAGE_TEXTURE_OPTIONS,
    );
    let item_id = app.current_edit_result_key(1).item_id;
    app.bind_display_texture_identity(&texture, item_id);
    let FsCacheEntry::Static {
        pixels: source,
        tex,
        ..
    } = app.fs_cache.get_mut(&1).unwrap()
    else {
        unreachable!()
    };
    *source = pixels.clone();
    *tex = texture;
    pixels
}

fn effective_local(size: [usize; 2]) -> local_adjust_core::LocalAdjustmentLayers {
    let [w, h] = size;
    let alpha = (0..w * h)
        .map(|i| {
            if i % w < w * 3 / 4 && i / w < h * 3 / 4 {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    local_adjust_core::LocalAdjustmentLayers::new(vec![
        local_adjust_core::LocalAdjustmentLayer::new(
            "masked invert",
            local_adjust_core::LocalMask::Raster(local_adjust_core::RasterMask {
                width: w,
                height: h,
                alpha: Arc::new(alpha),
            }),
            local_adjust_core::LocalEffect::Invert(local_adjust_core::InvertParams {
                strength: 0.8,
            }),
        ),
    ])
}

fn conceal_snapshot(size: [usize; 2]) -> MaskSnapshot {
    let [w, h] = size;
    MaskSnapshot {
        bitmap: (0..w * h).map(|i| i % w > w / 3 && i / w > h / 3).collect(),
        shapes: Vec::new(),
        size,
    }
}

fn configure_conceal(app: &mut App) {
    app.conceal_pages.insert(1);
    app.settings.conceal_type = crate::conceal::ConcealType::WhiteFill;
    app.settings.conceal_fill_opacity_percent = 70;
    app.settings.conceal_fill_edge = crate::conceal::FillEdge::Sharp;
}

fn use_conceal_database(
    app: &crate::app::AppTestEnvForTest,
    input: &mut PreparedEditInput,
    mask: &MaskSnapshot,
) {
    let path = app.tmp.path().join("review-conceal.db");
    let db = crate::conceal_db::ConcealDb::open_at(&path).unwrap();
    db.set(
        &input.page_key,
        &mask.bitmap,
        &mask.shapes,
        mask.size[0],
        mask.size[1],
    )
    .unwrap();
    drop(db);
    input.conceal.as_mut().unwrap().0 = path;
}

fn bubble() -> Vec<comic_core::AnnotationObject> {
    vec![comic_core::AnnotationObject::new_bubble(
        1,
        (36.0, 48.0),
        comic_core::BubbleObject {
            shape: comic_core::BubbleShape::RoundRect {
                half_w: 15.0,
                half_h: 18.0,
                corner_px: 0.0,
            },
            fill: Some(comic_core::Rgba::new(220, 40, 75, 180)),
            outline: comic_core::StrokeStyle {
                width_px: 0.0,
                ..Default::default()
            },
            auto_size: false,
            ..Default::default()
        },
    )]
}

fn annotation(app: &mut App, doc: Vec<comic_core::AnnotationObject>) {
    app.comic_pages.insert(1);
    app.comic_docs.insert(app.page_path_key(1).unwrap(), doc);
    app.comic_fonts.seed_for_test(
        Some(Arc::new(comic_core::FontSet::new())),
        Default::default(),
    );
    app.comic_font_registry_loaded = true;
}

fn ordinary_edit(
    raw: &Arc<egui::ColorImage>,
    local: Option<&local_adjust_core::LocalAdjustmentLayers>,
    conceal: Option<(&MaskSnapshot, &crate::conceal::ConcealPreset)>,
) -> Arc<egui::ColorImage> {
    let request = EditSourceRequest {
        raw: raw.clone(),
        erase: EditLayer::Absent,
        local_adjust: local.map_or(EditLayer::Absent, |layers| {
            EditLayer::Materialize(LocalAdjustMaterialize {
                layers: layers.clone(),
            })
        }),
        conceal: conceal.map_or(EditLayer::Absent, |(mask, preset)| {
            EditLayer::Materialize(ConcealMaterialize {
                mask: mask.clone(),
                preset: preset.clone(),
            })
        }),
    };
    let EditSourceResult::Ready(result) =
        crate::edit_source::execute_edit_source(request, &Arc::new(AtomicBool::new(false)))
            .unwrap()
    else {
        panic!("ordinary edit must complete")
    };
    result.pixels
}

fn ordinary_final(
    pixels: Arc<egui::ColorImage>,
    params: &crate::adjustment::AdjustParams,
) -> Arc<egui::ColorImage> {
    let FinalCompositeResult::Ready { pixels, .. } = execute_final_composite(
        pixels,
        build_final_composite_plan_without_ai(params, None),
        &Arc::new(AtomicBool::new(false)),
    ) else {
        panic!("ordinary final must complete")
    };
    pixels
}

fn ordinary_comic(base: &egui::ColorImage, comic: &ComicInput) -> egui::ColorImage {
    let scale = base.size[0].max(base.size[1]) as f32
        / comic.canonical_size[0].max(comic.canonical_size[1]) as f32;
    let doc = comic_core::scale_scene(comic.doc.as_ref().unwrap(), scale);
    let layers = comic_core::bake_annotation_layers(
        &doc,
        base.size[0],
        base.size[1],
        comic.fonts.as_ref().unwrap(),
        &comic_core::StampImages::new(),
    );
    crate::comic_overlay::composite_annotation_layers(base, &layers)
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
            diagnostic: None,
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

#[test]
fn effective_masked_local_conceal_final_and_comic_match_independent_ordinary_pixels() {
    for nearest in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let raw = patterned_source(&mut app, &ctx);
        let layers = effective_local(raw.size);
        app.local_adjust_pages.insert(1);
        app.local_adjust_page_layers.insert(1, layers.clone());
        configure_conceal(&mut app);
        annotation(&mut app, bubble());
        app.adjustment_page_params.get_mut(&1).unwrap().post_filter = if nearest {
            crate::adjustment::PostFilter::Nearest
        } else {
            crate::adjustment::PostFilter::None
        };
        let mut input = app.prepared_edit_input(1).unwrap();
        let mask = conceal_snapshot(raw.size);
        use_conceal_database(&app, &mut input, &mask);
        let preset = &input.conceal.as_ref().unwrap().1;
        let local = ordinary_edit(&raw, Some(&layers), None);
        let edit = ordinary_edit(&raw, Some(&layers), Some((&mask, preset)));
        let without_local = ordinary_edit(&raw, None, Some((&mask, preset)));
        let reversed = ordinary_edit(&without_local, Some(&layers), None);
        // The fixture must distinguish omitted stages and reversed stage order.
        assert_ne!(local.as_ref(), raw.as_ref());
        assert_eq!(local.pixels[raw.size[0] - 1], raw.pixels[raw.size[0] - 1]);
        assert_ne!(edit.as_ref(), local.as_ref());
        assert_ne!(edit.as_ref(), without_local.as_ref());
        assert_ne!(edit.as_ref(), reversed.as_ref());
        let final_pixels = ordinary_final(edit.clone(), &input.params);
        let comic_pixels = ordinary_comic(&final_pixels, input.comic.as_ref().unwrap());
        assert_ne!(final_pixels.as_ref(), edit.as_ref());
        assert_ne!(&comic_pixels, final_pixels.as_ref());
        if !nearest {
            let final_before_conceal = ordinary_edit(
                &ordinary_final(local.clone(), &input.params),
                None,
                Some((&mask, preset)),
            );
            let comic_before_final = ordinary_final(
                Arc::new(ordinary_comic(&edit, input.comic.as_ref().unwrap())),
                &input.params,
            );
            assert_ne!(final_pixels.as_ref(), final_before_conceal.as_ref());
            assert_ne!(&comic_pixels, comic_before_final.as_ref());
        }
        let output = execute(&input, &Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(
            output.local.as_ref().unwrap().pixels.as_ref(),
            local.as_ref()
        );
        assert_eq!(output.conceal.as_ref().unwrap().as_ref(), edit.as_ref());
        assert_eq!(output.edit.as_ref().unwrap().as_ref(), edit.as_ref());
        assert_eq!(
            output.final_pixels.as_ref().unwrap().as_ref(),
            final_pixels.as_ref()
        );
        assert_eq!(
            output.comic.as_ref().unwrap().pixels.as_ref(),
            &comic_pixels
        );
        assert!(Arc::ptr_eq(
            output.edit.as_ref().unwrap(),
            output.conceal.as_ref().unwrap()
        ));
        let published_base = output.final_pixels.as_ref().unwrap().clone();
        let key = install(&mut app, &ctx, input, output);
        app.resolve_prepared_edit_for_display(&ctx, 1);
        assert_eq!(
            app.final_composite_cache[&key].pixels.as_ref(),
            final_pixels.as_ref()
        );
        assert_eq!(app.comic_cache[&1].pixels.as_ref(), &comic_pixels);
        let options = if nearest {
            egui::TextureOptions::NEAREST
        } else {
            DISPLAY_IMAGE_TEXTURE_OPTIONS
        };
        assert_eq!(app.comic_cache[&1].texture_options, options);
        assert_eq!(
            ctx.tex_manager()
                .read()
                .meta(app.comic_cache[&1].texture.id())
                .unwrap()
                .options,
            options
        );
        assert!(Arc::ptr_eq(
            &app.final_composite_cache[&key].pixels,
            &published_base
        ));
        assert!(Arc::ptr_eq(&app.comic_cache[&1].base, &published_base));
        assert!(app.conceal_cache[&1].texture.is_none());
        assert!(
            app.local_adjust_cache[&app.current_local_adjust_key(1)]
                .texture
                .is_none()
        );
        assert!(app.final_effect_pending.is_empty());
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    }
}

#[test]
fn conceal_only_matches_ordinary_pixels_and_publishes_shared_edit_arc() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    let raw = patterned_source(&mut app, &ctx);
    configure_conceal(&mut app);
    let mut input = app.prepared_edit_input(1).unwrap();
    assert!(input.local.is_none());
    let mask = conceal_snapshot(raw.size);
    use_conceal_database(&app, &mut input, &mask);
    let edit = ordinary_edit(
        &raw,
        None,
        Some((&mask, &input.conceal.as_ref().unwrap().1)),
    );
    let expected = ordinary_final(edit.clone(), &input.params);
    assert_ne!(edit.as_ref(), raw.as_ref());
    let output = execute(&input, &Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(output.conceal.as_ref().unwrap().as_ref(), edit.as_ref());
    assert_eq!(
        output.final_pixels.as_ref().unwrap().as_ref(),
        expected.as_ref()
    );
    let conceal = output.conceal.as_ref().unwrap().clone();
    let base = output.final_pixels.as_ref().unwrap().clone();
    let key = install(&mut app, &ctx, input, output);
    app.resolve_prepared_edit_for_display(&ctx, 1);
    assert!(app.local_adjust_cache.is_empty());
    assert!(app.conceal_cache[&1].texture.is_none());
    assert!(Arc::ptr_eq(&app.conceal_cache[&1].pixels, &conceal));
    assert!(Arc::ptr_eq(
        &app.edit_result_cache[&key.edit_key].pixels,
        &conceal
    ));
    assert!(Arc::ptr_eq(&app.final_composite_cache[&key].pixels, &base));
    assert_eq!(
        app.final_composite_cache[&key].pixels.as_ref(),
        expected.as_ref()
    );
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
}

#[test]
fn successful_annotation_only_raw_and_completed_final_keep_base_arc_and_shared_handle() {
    for completed in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let raw = patterned_source(&mut app, &ctx);
        let (base, texture) = if completed {
            let texture = app.ensure_final_composite_texture(&ctx, 1).unwrap();
            let base = app
                .exact_complete_final_composite_entry(1)
                .unwrap()
                .1
                .pixels
                .clone();
            assert_eq!(
                base.as_ref(),
                ordinary_final(raw.clone(), app.effective_params(1)).as_ref()
            );
            (base, texture)
        } else {
            app.adjustment_page_params.get_mut(&1).unwrap().brightness = 0.0;
            let FsCacheEntry::Static { tex, .. } = app.fs_cache.get(&1).unwrap() else {
                unreachable!()
            };
            (raw.clone(), tex.clone())
        };
        annotation(&mut app, bubble());
        let input = app.prepared_edit_input(1).unwrap();
        assert!(matches!(input.output, PreparedOutput::AnnotationOnly));
        assert_eq!(
            matches!(input.final_input, PreparedFinalInput::Completed { .. }),
            completed
        );
        let expected = ordinary_comic(&base, input.comic.as_ref().unwrap());
        assert_ne!(&expected, base.as_ref());
        let output = execute(&input, &Arc::new(AtomicBool::new(false))).unwrap();
        assert!(output.local.is_none());
        assert!(output.conceal.is_none());
        assert_eq!(output.comic.as_ref().unwrap().pixels.as_ref(), &expected);
        if completed {
            assert!(output.final_pixels.is_none());
        } else {
            assert!(Arc::ptr_eq(output.final_pixels.as_ref().unwrap(), &raw));
            assert!(Arc::ptr_eq(output.edit.as_ref().unwrap(), &raw));
        }
        let key = install(&mut app, &ctx, input, output);
        app.resolve_prepared_edit_for_display(&ctx, 1);
        assert_eq!(app.final_composite_cache[&key].texture.id(), texture.id());
        assert!(Arc::ptr_eq(&app.final_composite_cache[&key].pixels, &base));
        assert!(Arc::ptr_eq(&app.comic_cache[&1].base, &base));
        assert_eq!(app.comic_cache[&1].pixels.as_ref(), &expected);
        assert!(app.prepared_edit_input(1).is_none());
        assert!(app.final_effect_pending.is_empty());
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    }
}

fn partially_loaded_fonts(app: &mut App) -> Vec<comic_core::AnnotationObject> {
    // Use egui's embedded font, with no OS font enumeration or file read.
    let definitions = egui::FontDefinitions::default();
    let font = definitions.font_data.values().next().unwrap();
    let mut fonts = comic_core::FontSet::new();
    fonts.insert(
        comic_core::LoadedFont::from_bytes("available", font.font.as_ref().as_ref().to_vec())
            .unwrap(),
    );
    app.comic_fonts.seed_for_test(
        Some(Arc::new(fonts)),
        HashSet::from(["available".to_owned()]),
    );
    ["available", "missing"]
        .into_iter()
        .enumerate()
        .map(|(i, key)| {
            comic_core::AnnotationObject::new_text(
                i as u64 + 1,
                (5.0, 10.0 + i as f32 * 20.0),
                comic_core::TextBlock {
                    text: "font".to_owned(),
                    font_key: key.to_owned(),
                    ..Default::default()
                },
            )
        })
        .collect()
}

fn cold_document(
    app: &crate::app::AppTestEnvForTest,
    input: &mut PreparedEditInput,
    doc: &[comic_core::AnnotationObject],
) {
    let path = app.tmp.path().join("review-cold-comic.db");
    let db = crate::comic_db::ComicDb::open_at(&path).unwrap();
    db.set(&input.page_key, doc).unwrap();
    drop(db);
    input.comic.as_mut().unwrap().path = path;
}

#[test]
fn cold_doc_with_one_missing_font_is_hydrated_once_without_publishing_partial_annotation() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    patterned_source(&mut app, &ctx);
    let texture = app.ensure_final_composite_texture(&ctx, 1).unwrap();
    let base = app
        .exact_complete_final_composite_entry(1)
        .unwrap()
        .1
        .pixels
        .clone();
    app.comic_pages.insert(1);
    let doc = partially_loaded_fonts(&mut app);
    assert!(!fonts_satisfy(
        &doc,
        app.comic_fonts.fonts(),
        app.comic_fonts.loaded_keys()
    ));
    assert!(fonts_satisfy(
        &doc[..1],
        app.comic_fonts.fonts(),
        app.comic_fonts.loaded_keys()
    ));
    let mut input = app.prepared_edit_input(1).unwrap();
    cold_document(&app, &mut input, &doc);
    let page_key = input.page_key.clone();
    let output = execute(&input, &Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(output.doc.as_ref().unwrap(), &doc);
    assert!(output.comic.is_none());
    assert!(output.final_pixels.is_none());
    let key = install(&mut app, &ctx, input, output);
    app.resolve_prepared_edit_for_display(&ctx, 1);
    assert_eq!(app.comic_docs[&page_key], doc);
    assert!(app.comic_cache.is_empty());
    assert!(Arc::ptr_eq(&app.final_composite_cache[&key].pixels, &base));
    assert_eq!(app.final_composite_cache[&key].texture.id(), texture.id());
    assert!(app.prepared_edit_input(1).is_none());
    assert!(app.final_effect_pending.is_empty());
    assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
}

#[test]
fn conceal_and_final_publish_when_a_cached_or_cold_doc_has_one_missing_font() {
    for cold in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let raw = patterned_source(&mut app, &ctx);
        configure_conceal(&mut app);
        app.comic_pages.insert(1);
        let doc = partially_loaded_fonts(&mut app);
        let page_key = app.page_path_key(1).unwrap();
        if !cold {
            app.comic_docs.insert(page_key.clone(), doc.clone());
        }
        let mut input = app.prepared_edit_input(1).unwrap();
        assert_eq!(input.comic.is_some(), cold);
        if cold {
            cold_document(&app, &mut input, &doc);
        }
        let mask = conceal_snapshot(raw.size);
        use_conceal_database(&app, &mut input, &mask);
        let edit = ordinary_edit(
            &raw,
            None,
            Some((&mask, &input.conceal.as_ref().unwrap().1)),
        );
        let expected = ordinary_final(edit.clone(), &input.params);
        let output = execute(&input, &Arc::new(AtomicBool::new(false))).unwrap();
        assert!(output.comic.is_none());
        assert_eq!(output.conceal.as_ref().unwrap().as_ref(), edit.as_ref());
        assert_eq!(
            output.final_pixels.as_ref().unwrap().as_ref(),
            expected.as_ref()
        );
        let key = install(&mut app, &ctx, input, output);
        app.resolve_prepared_edit_for_display(&ctx, 1);
        assert_eq!(app.comic_docs[&page_key], doc);
        assert!(app.comic_cache.is_empty());
        assert_eq!(app.conceal_cache[&1].pixels.as_ref(), edit.as_ref());
        assert!(Arc::ptr_eq(
            &app.conceal_cache[&1].pixels,
            &app.edit_result_cache[&key.edit_key].pixels
        ));
        assert_eq!(
            app.final_composite_cache[&key].pixels.as_ref(),
            expected.as_ref()
        );
        assert!(app.prepared_edit_input(1).is_none());
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
    }
}

#[test]
fn full_comic_capacity_skips_annotation_only_but_preserves_lower_stage_publication() {
    for lower_stage in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let raw = patterned_source(&mut app, &ctx);
        app.adjustment_page_params.get_mut(&1).unwrap().brightness = 0.0;
        annotation(&mut app, bubble());
        if lower_stage {
            configure_conceal(&mut app);
        }
        app.comic_bake_inflight.store(2, Ordering::Relaxed);
        if !lower_stage {
            assert!(app.prepared_edit_input(1).is_none());
            assert!(app.final_effect_pending.is_empty());
            assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
        } else {
            let mut input = app.prepared_edit_input(1).unwrap();
            assert!(input.comic.is_none());
            let mask = conceal_snapshot(raw.size);
            use_conceal_database(&app, &mut input, &mask);
            let expected = ordinary_edit(
                &raw,
                None,
                Some((&mask, &input.conceal.as_ref().unwrap().1)),
            );
            let output = execute(&input, &Arc::new(AtomicBool::new(false))).unwrap();
            assert!(output.comic.is_none());
            let key = install(&mut app, &ctx, input, output);
            app.resolve_prepared_edit_for_display(&ctx, 1);
            assert_eq!(
                app.final_composite_cache[&key].pixels.as_ref(),
                expected.as_ref()
            );
            assert!(Arc::ptr_eq(
                &app.final_composite_cache[&key].pixels,
                &app.conceal_cache[&1].pixels
            ));
            assert!(app.comic_cache.is_empty());
            assert!(app.prepared_edit_input(1).is_none());
            assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
        }
        assert_eq!(app.comic_bake_inflight.load(Ordering::Relaxed), 2);
        app.comic_bake_inflight.store(0, Ordering::Relaxed);
    }
}

fn set_sampler(app: &mut App, nearest: bool) -> egui::TextureOptions {
    app.adjustment_page_params.get_mut(&1).unwrap().post_filter = if nearest {
        crate::adjustment::PostFilter::Nearest
    } else {
        crate::adjustment::PostFilter::None
    };
    if nearest {
        egui::TextureOptions::NEAREST
    } else {
        DISPLAY_IMAGE_TEXTURE_OPTIONS
    }
}

fn texture_texels(texture: &egui::TextureHandle, options: egui::TextureOptions) -> usize {
    crate::vram_budget::texture_size_texels(texture.size(), options.mipmap_mode.is_some())
}

#[test]
fn annotation_private_to_public_keeps_actual_sampler_handle_and_vram_texels() {
    use crate::app::final_prepare::tests::begin_registration_frame;
    use crate::vram_budget::VramSubsystem;

    let mut registered_texels = Vec::new();
    for nearest in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        patterned_source(&mut app, &ctx);
        let options = set_sampler(&mut app, nearest);
        annotation(&mut app, bubble());
        let input = app.prepared_edit_input(1).unwrap();
        let output = execute(&input, &Arc::new(AtomicBool::new(false))).unwrap();
        let key = install(&mut app, &ctx, input, output);
        let before = app.vram_accounting_snapshot().total_texels;
        begin_registration_frame(&mut app, 2001);
        assert!(app.adopt_prepared_edit(
            &ctx,
            key,
            FinalPrepareAdoption::RootFinalized(
                None,
                crate::ui_fullscreen::FinalUnitPosition::Next
            ),
            false,
        ));
        let FinalEffectState::Registering(registration) =
            &mut app.final_effect_pending.get_mut(&key).unwrap().state
        else {
            panic!("first ROOT frame must retain the private final texture")
        };
        let final_texture = registration.final_texture.as_ref().unwrap().clone();
        assert!(registration.comic_texture.is_none());
        assert_eq!(
            ctx.tex_manager()
                .read()
                .meta(final_texture.id())
                .unwrap()
                .options,
            options
        );
        // Observe the last private state directly: production publishes immediately
        // after registering this second handle, without an intervening callback.
        let comic_texture = ctx.load_texture(
            "review-private-comic",
            registration
                .ready
                .output
                .as_ref()
                .unwrap()
                .comic
                .as_ref()
                .unwrap()
                .upload
                .clone(),
            options,
        );
        registration.comic_texture = Some(comic_texture.clone());
        assert!(app.comic_cache.is_empty());
        assert!(app.final_composite_cache.is_empty());
        let final_texels = texture_texels(&final_texture, options);
        let comic_texels = texture_texels(&comic_texture, options);
        let private = app.vram_accounting_snapshot();
        assert_eq!(private.total_texels, before + final_texels + comic_texels);
        assert_eq!(private.subsystem_texels(VramSubsystem::ComicCache), 0);
        app.resolve_prepared_edit_for_display(&ctx, 1);
        let published = app.vram_accounting_snapshot();
        assert_eq!(published.total_texels, private.total_texels);
        assert_eq!(
            published.subsystem_texels(VramSubsystem::ComicCache),
            comic_texels
        );
        assert_eq!(
            app.final_composite_cache[&key].texture.id(),
            final_texture.id()
        );
        assert_eq!(app.comic_cache[&1].texture.id(), comic_texture.id());
        assert_eq!(app.comic_cache[&1].texture_options, options);
        assert_eq!(
            ctx.tex_manager()
                .read()
                .meta(comic_texture.id())
                .unwrap()
                .options,
            options
        );
        assert!(app.final_effect_pending.is_empty());
        assert_eq!(app.final_prepare_cpu.snapshot().bytes, 0);
        registered_texels.push(comic_texels);
    }
    assert!(
        registered_texels[0] > registered_texels[1],
        "only LINEAR includes mip texels"
    );
}

#[test]
fn ordinary_sync_and_worker_annotation_producers_preserve_actual_texture_options() {
    use crate::vram_budget::VramSubsystem;

    for synchronous in [false, true] {
        let mut registered_texels = Vec::new();
        for nearest in [false, true] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx);
            patterned_source(&mut app, &ctx);
            let options = set_sampler(&mut app, nearest);
            app.ensure_final_composite_texture(&ctx, 1).unwrap();
            annotation(&mut app, bubble());
            app.text_mode = synchronous;
            app.settings.text_preview_scale = 1;
            let before = app.vram_accounting_snapshot().total_texels;
            let texture = app.ensure_comic_composite_texture(&ctx, 1);
            if synchronous {
                assert!(texture.is_some());
                assert!(app.comic_bake_pending.is_empty());
            } else {
                assert!(texture.is_none());
                // Receive the actual ordinary worker result, then present it to
                // the production nonblocking poll through a controlled channel.
                let result = app.comic_bake_pending[&1]
                    .rx
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
                let (tx, rx) = mpsc::channel();
                tx.send(result)
                    .unwrap_or_else(|_| panic!("connected comic completion channel"));
                app.comic_bake_pending.get_mut(&1).unwrap().rx = rx;
                app.poll_comic_bake(&ctx);
                assert!(app.comic_bake_pending.is_empty());
            }
            let entry = &app.comic_cache[&1];
            assert_eq!(entry.texture_options, options);
            assert_eq!(
                ctx.tex_manager()
                    .read()
                    .meta(entry.texture.id())
                    .unwrap()
                    .options,
                options
            );
            let texels = texture_texels(&entry.texture, options);
            let snapshot = app.vram_accounting_snapshot();
            assert_eq!(snapshot.subsystem_texels(VramSubsystem::ComicCache), texels);
            assert_eq!(snapshot.total_texels, before + texels);
            assert!(app.final_effect_pending.is_empty());
            registered_texels.push(texels);
        }
        assert!(
            registered_texels[0] > registered_texels[1],
            "only LINEAR includes mip texels"
        );
    }
}
