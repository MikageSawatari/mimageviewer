//! Resident edit-stage selection and immutable display-allocation sharing.
use super::phase_c_support::setup_app;
use super::source_texture_test_support::seed;
use super::*;

fn image(app: &mut App, ctx: &egui::Context) {
    app.items
        .push(GridItem::Image(PathBuf::from("C:/resident-edit/0.png")));
    app.thumbnails.push(ThumbnailState::Pending);
    seed(app, ctx, 0);
}

fn stage(
    app: &mut App,
    ctx: &egui::Context,
    name: &str,
    options: egui::TextureOptions,
) -> (
    Arc<egui::ColorImage>,
    egui::TextureId,
    crate::material_display_basis::MaterialDisplayBasis,
) {
    let pixels = Arc::new(egui::ColorImage::filled([4, 6], egui::Color32::GOLD));
    let texture = ctx.load_texture(name, pixels.clone(), options);
    app.bind_display_texture_identity(&texture, app.item_id(0));
    let id = texture.id();
    let display_basis = crate::material_display_basis::MaterialDisplayBasis::from_sampler(
        [123, 456],
        crate::source_coverage::SourceCoverage::FullSource,
    );
    match name {
        "erase" => {
            app.mask_pages.insert(0);
            app.erase_result_cache.insert(
                app.current_erase_result_key(0),
                EraseResultCacheEntry {
                    texture_options: options,
                    display_basis,
                    pixels: pixels.clone(),
                    texture,
                },
            );
        }
        "local_adjust" => {
            app.local_adjust_pages.insert(0);
            app.local_adjust_page_layers.insert(
                0,
                local_adjust_core::LocalAdjustmentLayers::new(vec![
                    local_adjust_core::LocalAdjustmentLayer::new(
                        "ready",
                        local_adjust_core::LocalMask::Full,
                        local_adjust_core::LocalEffect::None,
                    ),
                ]),
            );
            app.local_adjust_cache.insert(
                app.current_local_adjust_key(0),
                LocalAdjustCacheEntry {
                    texture_options: options,
                    display_basis,
                    pixels: pixels.clone(),
                    texture,
                },
            );
        }
        "conceal" => {
            app.conceal_pages.insert(0);
            app.conceal_cache.insert(
                0,
                ConcealCacheEntry {
                    texture_options: options,
                    display_basis,
                    pixels: pixels.clone(),
                    texture,
                    generation: app.conceal_generation,
                },
            );
        }
        _ => unreachable!(),
    }
    (pixels, id, display_basis)
}

#[test]
fn resident_edit_stage_uses_exact_highest_material_and_keeps_its_basis() {
    let ctx = egui::Context::default();
    let mut app = setup_app();
    image(&mut app, &ctx);
    let (_, erase_id, _) = stage(&mut app, &ctx, "erase", DISPLAY_IMAGE_TEXTURE_OPTIONS);
    let (_, local_id, _) = stage(
        &mut app,
        &ctx,
        "local_adjust",
        DISPLAY_IMAGE_TEXTURE_OPTIONS,
    );
    let (pixels, conceal_id, basis) =
        stage(&mut app, &ctx, "conceal", DISPLAY_IMAGE_TEXTURE_OPTIONS);
    let (material, selected_pixels, label) = app.exact_resident_edit_stage(0).unwrap();
    assert_eq!(label, "conceal");
    assert_eq!(material.source_texture().id(), conceal_id);
    assert_eq!(material.display_basis(), basis);
    assert!(Arc::ptr_eq(&selected_pixels, &pixels));
    drop(material);
    app.conceal_generation += 1;
    let (material, _, label) = app.exact_resident_edit_stage(0).unwrap();
    assert_eq!(
        (material.source_texture().id(), label),
        (local_id, "local_adjust")
    );
    drop(material);
    app.local_adjust_generation.insert(0, 1);
    let (material, _, label) = app.exact_resident_edit_stage(0).unwrap();
    assert_eq!((material.source_texture().id(), label), (erase_id, "erase"));
    drop(material);
    app.erase_mask_generation.insert(0, 1);
    assert!(app.exact_resident_edit_stage(0).is_none());
    assert!(app.local_adjust_pending.is_empty());
    assert!(app.erase_inpaint_pending.is_empty());
    assert!(app.edit_result_cache.is_empty());
}

#[test]
fn resident_edit_linear_stages_share_with_edit_final_and_count_once() {
    for name in ["erase", "local_adjust", "conceal"] {
        for cpu_prefetched in [false, true] {
            let ctx = egui::Context::default();
            let mut app = setup_app();
            image(&mut app, &ctx);
            let (pixels, id, basis) = stage(&mut app, &ctx, name, DISPLAY_IMAGE_TEXTURE_OPTIONS);
            let key = app.current_edit_result_key(0);
            {
                let actual = app
                    .shared_resident_texture_for_edit(&ctx, &key, &pixels)
                    .unwrap();
                assert_eq!(actual.source_texture().id(), id);
                assert_eq!(actual.display_basis(), basis);
                let prewarm = app.resident_final_prepare_material(&ctx, 0).unwrap();
                assert_eq!(prewarm.source_texture().id(), id);
                assert_eq!(prewarm.display_basis(), basis);
            }
            if cpu_prefetched {
                let (returned_key, input) = app.ensure_edit_result_pixels_cpu(&ctx, 0).unwrap();
                assert_eq!(returned_key, key);
                assert!(Arc::ptr_eq(&input, &pixels));
                assert!(app.edit_result_cache[&key].texture.is_none());
                assert_eq!(app.edit_result_cache[&key].display_basis, basis);
            }
            ctx.tex_manager().write().take_delta().clear();
            let budget = app.last_edit_materialize_upload_frame;
            let final_texture = app.ensure_final_composite_texture(&ctx, 0).unwrap();
            assert_eq!(final_texture.id(), id, "{name} cpu={cpu_prefetched}");
            assert_eq!(
                app.edit_result_cache[&key].texture.as_ref().unwrap().id(),
                id
            );
            assert!(Arc::ptr_eq(&app.edit_result_cache[&key].pixels, &pixels));
            assert_eq!(app.edit_result_cache[&key].display_basis, basis);
            assert_eq!(app.last_edit_materialize_upload_frame, budget);
            assert!(ctx.tex_manager().write().take_delta().is_empty());
            assert_eq!(
                App::resident_texture_options(&ctx, &final_texture),
                DISPLAY_IMAGE_TEXTURE_OPTIONS
            );
            drop(final_texture);
            let stage_texels = crate::vram_budget::texture_size_texels([4, 6], true);
            assert_eq!(
                app.vram_accounting_snapshot().total_texels,
                stage_texels * 2
            );
            // Drop each owner independently. The shared immutable allocation
            // belongs to edit/final until their last handle is released.
            app.fs_cache.clear();
            ctx.tex_manager().write().take_delta().clear();
            app.erase_result_cache.clear();
            app.local_adjust_cache.clear();
            app.conceal_cache.clear();
            assert_eq!(app.vram_accounting_snapshot().total_texels, stage_texels);
            assert!(ctx.tex_manager().write().take_delta().is_empty());
            app.edit_result_cache.clear();
            assert_eq!(app.vram_accounting_snapshot().total_texels, stage_texels);
            assert!(ctx.tex_manager().write().take_delta().is_empty());
            app.final_composite_cache.clear();
            assert_eq!(app.vram_accounting_snapshot().total_texels, 0);
            assert!(ctx.tex_manager().read().meta(id).is_none());
        }
    }
}

#[test]
fn resident_edit_nearest_stage_is_not_reused_as_a_linear_edit_or_final() {
    for name in ["erase", "local_adjust", "conceal"] {
        let ctx = egui::Context::default();
        let mut app = setup_app();
        image(&mut app, &ctx);
        let (pixels, id, _) = stage(&mut app, &ctx, name, egui::TextureOptions::NEAREST);
        let key = app.current_edit_result_key(0);
        assert!(
            app.shared_resident_texture_for_edit(&ctx, &key, &pixels)
                .is_none()
        );
        app.frame_counter += 1;
        drop(app.ensure_final_composite_texture(&ctx, 0).unwrap());
        let edit = app.edit_result_cache[&key].texture.as_ref().unwrap();
        assert_ne!(edit.id(), id);
        assert_eq!(
            App::resident_texture_options(&ctx, edit),
            DISPLAY_IMAGE_TEXTURE_OPTIONS
        );
        assert_eq!(
            app.current_final_composite_texture(0).unwrap().id(),
            edit.id()
        );
        assert_eq!(
            App::resident_texture_options(
                &ctx,
                app.exact_resident_edit_stage(0).unwrap().0.source_texture()
            ),
            egui::TextureOptions::NEAREST
        );
        let expected = crate::vram_budget::texture_size_texels([4, 6], true) * 2 + 4 * 6;
        assert_eq!(
            app.vram_accounting_snapshot().total_texels,
            expected,
            "nearest stage has no mip chain"
        );
    }
}

#[test]
fn resident_edit_sharing_requires_same_arc_dimensions_key_and_item_identity() {
    for name in ["erase", "local_adjust", "conceal"] {
        for mismatch in ["arc", "dimensions", "key", "identity"] {
            let ctx = egui::Context::default();
            let mut app = setup_app();
            image(&mut app, &ctx);
            let (pixels, _, _) = stage(&mut app, &ctx, name, DISPLAY_IMAGE_TEXTURE_OPTIONS);
            let mut key = app.current_edit_result_key(0);
            let mut input = pixels.clone();
            match mismatch {
                "arc" => input = Arc::new((*pixels).clone()),
                "key" => key.local_gen += 1,
                "identity" => {
                    app.items
                        .push(GridItem::Image(PathBuf::from("C:/resident-edit/other.png")));
                    let texture = app
                        .exact_resident_edit_stage(0)
                        .unwrap()
                        .0
                        .source_texture()
                        .clone();
                    *app.texture_identity.borrow_mut() = Default::default();
                    app.bind_display_texture_identity(&texture, app.item_id(1));
                }
                _ => {
                    let texture = ctx.load_texture(
                        "wrong-size",
                        egui::ColorImage::filled([2, 3], egui::Color32::WHITE),
                        DISPLAY_IMAGE_TEXTURE_OPTIONS,
                    );
                    app.bind_display_texture_identity(&texture, app.item_id(0));
                    match name {
                        "erase" => {
                            let key = app.current_erase_result_key(0);
                            app.erase_result_cache.get_mut(&key).unwrap().texture = texture
                        }
                        "local_adjust" => {
                            let key = app.current_local_adjust_key(0);
                            app.local_adjust_cache.get_mut(&key).unwrap().texture = texture
                        }
                        _ => app.conceal_cache.get_mut(&0).unwrap().texture = texture,
                    }
                }
            }
            let _expected = crate::item_identity::expect_mismatch();
            assert!(
                app.shared_resident_texture_for_edit(&ctx, &key, &input)
                    .is_none(),
                "{name} {mismatch}"
            );
        }
    }
}
