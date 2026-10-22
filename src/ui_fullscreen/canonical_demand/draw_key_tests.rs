use super::tests::{accept_current, actual_transforms, ready_app, view};
use super::*;
use crate::gpu_lanczos::{
    LanczosCacheKey, NativePrewarmExecution, NativePrewarmPlan, NativePrewarmQuery,
    begin_native_lookup_observation_for_test, take_native_lookups_for_test,
};

fn render_state() -> egui_wgpu::RenderState {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let target_format = wgpu::TextureFormat::Rgba8Unorm;
    let renderer = egui_wgpu::Renderer::new(&device, target_format, Default::default());
    egui_wgpu::RenderState {
        instance,
        adapter,
        available_adapters: Vec::new(),
        device,
        queue,
        target_format,
        renderer: Arc::new(egui::epaint::mutex::RwLock::new(renderer)),
    }
}

fn deliver(app: &App, ctx: &egui::Context) {
    let state = app.wgpu_render_state.as_ref().unwrap();
    for idx in 0..app.items.len() {
        let resource = app.resident_canonical_resource(ctx, idx).unwrap();
        state.renderer.write().update_texture(
            &state.device,
            &state.queue,
            resource.source_texture_id(),
            &egui::epaint::ImageDelta::full(
                egui::ColorImage::filled(resource.size(), egui::Color32::WHITE),
                crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS,
            ),
        );
    }
}

/// A real owning UI callback, including a viewport-local physical scale and a
/// restricted painter clip. No candidate values are passed to ordinary draw.
fn callback(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    size: egui::Vec2,
    ppp: f32,
    clip_inset: egui::Vec2,
    mut run: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    let full = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
    let mut raw = egui::RawInput {
        viewport_id: viewport,
        screen_rect: Some(full),
        ..Default::default()
    };
    raw.viewports.insert(
        viewport,
        egui::ViewportInfo {
            parent: (viewport != egui::ViewportId::ROOT).then_some(egui::ViewportId::ROOT),
            native_pixels_per_point: Some(ppp),
            ..Default::default()
        },
    );
    crate::test_ui::headless_paint_output(ctx.run_pass(raw, |ctx| {
        assert_eq!(ctx.viewport_id(), viewport);
        assert_eq!(ctx.pixels_per_point(), ppp);
        ctx.root_ui(|root| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(root, |ui| {
                    ui.set_clip_rect(ui.max_rect().shrink2(clip_inset));
                    run(ui, ctx);
                });
        });
    }));
}

fn capture(app: &mut App, ui: &egui::Ui) -> CanonicalUnitDemand {
    app.record_next_canonical_native_demand(ui);
    let demand = app
        .collect_next_canonical_demand(ui.ctx(), view(ui))
        .unwrap();
    assert!(app.canonical_native_demand_was_observed(&demand));
    assert!(app.canonical_unit_demand_is_current(ui.ctx(), &demand));
    demand
}

fn query(app: &App, ctx: &egui::Context, page: &CanonicalPageDemand) -> NativePrewarmPlan {
    let resource = app
        .resident_canonical_resource(ctx, page.occurrence.idx)
        .unwrap();
    match app.fs_lanczos_cache.query_prewarm(
        app.wgpu_render_state.as_ref(),
        &resource,
        page.transform.total_scale,
        page.transform.pixels_per_point,
        page.visible_region,
        page.post_filter,
        page.smoothing_percent,
        page.anime_source_limit,
    ) {
        NativePrewarmQuery::Miss(plan) => plan,
        other => panic!("expected a new Native key, got {other:?}"),
    }
}

fn prewarm(app: &mut App, ctx: &egui::Context, page: &CanonicalPageDemand) -> LanczosCacheKey {
    let plan = query(app, ctx, page);
    let resource = app
        .resident_canonical_resource(ctx, page.occurrence.idx)
        .unwrap();
    assert!(matches!(
        app.fs_lanczos_cache.execute_prewarm(
            app.wgpu_render_state.as_ref().unwrap(),
            &resource,
            plan,
            |_| true,
        ),
        NativePrewarmExecution::Submitted { .. }
    ));
    plan.key()
}

fn assert_draw_keys(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    demand: &CanonicalUnitDemand,
    keys: &[LanczosCacheKey],
    expected_outcome: &str,
) {
    begin_native_lookup_observation_for_test();
    let actual = actual_transforms(app, ui, ctx, demand);
    assert_eq!(
        actual,
        demand
            .pages
            .iter()
            .map(|page| page.transform)
            .collect::<Vec<_>>()
    );
    let lookups = take_native_lookups_for_test();
    assert_eq!(
        lookups.len(),
        keys.len(),
        "one ordinary resolve per actual page: {lookups:?}"
    );
    for ((lookup, key), page) in lookups.iter().zip(keys).zip(&demand.pages) {
        assert_eq!(lookup.source_texture_id, page.source_id);
        assert_eq!(
            lookup.key,
            Some(*key),
            "ordinary painter clip/ROI must match prewarm"
        );
        assert_eq!(lookup.outcome, expected_outcome);
        let roi = key
            .visible_source_uv_rect()
            .expect("fixture must exercise Native ROI");
        assert!(roi.is_positive());
        assert!(
            roi.width() < 0.99 || roi.height() < 0.99,
            "fixture must be partially clipped"
        );
    }
}

fn ready_pdf_app(ctx: &egui::Context, mode: SpreadMode) -> crate::app::AppTestEnvForTest {
    // Establish PDF identities before the first accepted presentation. Reusing
    // an Image fixture after acceptance would retain foreign source leases.
    let mut app = super::super::page_turn_submission_tests::ready_app(ctx, mode);
    app.settings.page_turn_animation_enabled = true;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    app.settings.margin_fit_enabled = false;
    app.settings.still_seek_strip_visible = false;
    app.view_trim_apply_mode = crate::view_trim::ViewTrimApplyMode::None;
    // Canonical PDF page boxes deliberately differ in aspect and units from the
    // resident 80x120 raster. This catches substitution of sampled texture size.
    for idx in 0..4 {
        app.items[idx] = GridItem::PdfPage {
            pdf_path: PathBuf::from("c:/canonical-key-fixture/book.pdf"),
            page_num: idx as u32,
            content_type: Some(crate::pdf_loader::PdfPageContentType::Vector),
        };
        let FsCacheEntry::Static { pixels, .. } = app.fs_cache.get(&idx).unwrap() else {
            panic!("static PDF raster fixture");
        };
        let pixels = pixels.clone();
        let tex = ctx.load_texture(
            format!("canonical-pdf-source-{idx}"),
            pixels.clone(),
            crate::app::DISPLAY_IMAGE_TEXTURE_OPTIONS,
        );
        app.bind_display_texture_identity(&tex, app.item_id(idx));
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                pdf_rerender_failures: Vec::new(),
                pdf_metadata: Some(crate::app::PdfPageMetadataSnapshot {
                    layout_dims: (600_000, 850_000),
                    content_type: crate::pdf_loader::PdfPageContentType::Vector,
                }),
                tex,
                pixels,
                source_dims: Some([80, 120]),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
        app.record_page_layout_dims_for_spread(idx, (600_000, 850_000));
        assert_ne!(
            app.fs_page_layout_source_size(idx).unwrap(),
            app.resident_canonical_resource(ctx, idx)
                .unwrap()
                .size_vec2()
        );
    }
    accept_current(&mut app, ctx);
    app
}

fn specified_height(app: &mut App) {
    app.settings.fullscreen_fit_mode = FullscreenFitMode::SpecifiedScale;
    app.settings.fullscreen_specified_scale = crate::settings::FullscreenSpecifiedScale {
        width: crate::settings::SpecifiedScaleAxisSetting::disabled(
            crate::settings::SpecifiedScaleAxis::PercentTenths(1000),
        ),
        height: crate::settings::SpecifiedScaleAxisSetting::enabled(
            crate::settings::SpecifiedScaleAxis::LogicalPixels(640),
        ),
    };
}

#[test]
fn d3_4b_actual_single_pair_draw_native_keys_match_prewarm_with_pdf_specified_and_child_dpi() {
    let state = render_state();
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        for pdf in [false, true] {
            for specified in [false, true] {
                let ctx = egui::Context::default();
                let mut app = if pdf {
                    ready_pdf_app(&ctx, mode)
                } else {
                    ready_app(&ctx, mode)
                };
                if specified {
                    specified_height(&mut app);
                }
                app.wgpu_render_state = Some(state.clone());
                deliver(&app, &ctx);
                let child = egui::ViewportId::from_hash_of("canonical-key-child");
                let mut captured = None;
                callback(
                    &ctx,
                    child,
                    egui::vec2(600.0, 400.0),
                    1.5,
                    egui::vec2(125.0, 70.0),
                    |ui, ctx| {
                        let demand = capture(&mut app, ui);
                        assert_eq!(demand.view.pixels_per_point, 1.5);
                        let keys = demand
                            .pages
                            .iter()
                            .map(|page| prewarm(&mut app, ctx, page))
                            .collect::<Vec<_>>();
                        captured = Some((demand, keys));
                    },
                );
                let (demand, keys) = captured.unwrap();
                callback(
                    &ctx,
                    egui::ViewportId::ROOT,
                    egui::vec2(600.0, 400.0),
                    1.0,
                    egui::Vec2::ZERO,
                    |_, ctx| {
                        assert_eq!(ctx.pixels_per_point(), 1.0, "ROOT must retain its own DPI");
                        assert!(
                            app.canonical_unit_demand_is_current(ctx, &demand),
                            "ROOT replay must use the owning child's saved DPI"
                        );
                    },
                );
                callback(
                    &ctx,
                    child,
                    egui::vec2(600.0, 400.0),
                    1.5,
                    egui::vec2(125.0, 70.0),
                    |ui, ctx| {
                        assert_eq!(capture(&mut app, ui), demand);
                        assert_draw_keys(&mut app, ui, ctx, &demand, &keys, "hit");
                    },
                );
            }
        }
    }
}

#[test]
fn d3_4b_real_callback_resize_dpi_and_clip_replace_demand_and_old_native_keys_do_not_hit() {
    let state = render_state();
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        for change in ["resize", "dpi", "clip"] {
            let ctx = egui::Context::default();
            let mut app = ready_app(&ctx, mode);
            specified_height(&mut app);
            app.wgpu_render_state = Some(state.clone());
            deliver(&app, &ctx);
            let mut previous = None;
            let mut old_keys = Vec::new();
            callback(
                &ctx,
                egui::ViewportId::ROOT,
                egui::vec2(600.0, 400.0),
                1.0,
                egui::vec2(110.0, 60.0),
                |ui, ctx| {
                    let demand = capture(&mut app, ui);
                    old_keys = demand
                        .pages
                        .iter()
                        .map(|page| prewarm(&mut app, ctx, page))
                        .collect();
                    previous = Some(demand);
                },
            );
            let previous = previous.unwrap();
            let size = if change == "resize" {
                egui::vec2(520.0, 350.0)
            } else {
                egui::vec2(600.0, 400.0)
            };
            let ppp = if change == "dpi" { 1.5 } else { 1.0 };
            let inset = if change == "clip" {
                egui::vec2(145.0, 85.0)
            } else {
                egui::vec2(110.0, 60.0)
            };
            callback(&ctx, egui::ViewportId::ROOT, size, ppp, inset, |ui, ctx| {
                let current = capture(&mut app, ui);
                assert_ne!(
                    current, previous,
                    "actual {change} callback must replace the candidate"
                );
                assert!(
                    !app.canonical_unit_demand_is_current(ctx, &previous),
                    "old {change} demand must expire"
                );
                assert!(!app.canonical_native_demand_was_observed(&previous));
                let new_keys = current
                    .pages
                    .iter()
                    .map(|page| query(&app, ctx, page).key())
                    .collect::<Vec<_>>();
                assert!(
                    new_keys.iter().zip(&old_keys).all(|(new, old)| new != old),
                    "{change} must change actual key identity"
                );
                // Only the old request is cached. Ordinary painting must request
                // the new key and report miss rather than reuse the old output.
                assert_draw_keys(&mut app, ui, ctx, &current, &new_keys, "miss");
            });
        }
    }
}

#[test]
fn back_previous_native_matches_real_single_arrival_and_pair_draw_roi_keys() {
    let state = render_state();
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        let ctx = egui::Context::default();
        let mut app = ready_app(&ctx, mode);
        app.fullscreen_idx = Some(2);
        accept_current(&mut app, &ctx);
        app.prepare_final_demand_topologies();
        specified_height(&mut app);
        app.wgpu_render_state = Some(state.clone());
        deliver(&app, &ctx);
        callback(
            &ctx,
            egui::ViewportId::ROOT,
            egui::vec2(600.0, 400.0),
            1.5,
            egui::vec2(125.0, 70.0),
            |ui, ctx| {
                app.record_next_canonical_native_demand(ui);
                let previous = app
                    .collect_canonical_demand(ctx, view(ui), FinalUnitPosition::Previous)
                    .unwrap();
                assert!(app.canonical_unit_demand_is_current(ctx, &previous));
                let keys = previous
                    .pages
                    .iter()
                    .map(|page| prewarm(&mut app, ctx, page))
                    .collect::<Vec<_>>();
                assert_eq!(keys.len(), if mode == SpreadMode::Single { 1 } else { 2 });
                assert_draw_keys(&mut app, ui, ctx, &previous, &keys, "hit");
            },
        );
    }
}
