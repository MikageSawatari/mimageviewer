//! Held output owns a temporary view; ordinary complete output owns pan bounds.
use super::*;
use crate::gpu_lanczos::FullscreenPageMaterialKind;
use crate::ui_fullscreen::pan_bounds::draw_pan_correction;

fn draw(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    selected_current: bool,
) -> FsNavigatorTextureSources {
    let idx = app.fullscreen_idx.unwrap();
    let composition = app.spread_display_composition_for_anchor(idx);
    let screen = composition.pages_in_screen_order(app.spread_mode);
    if let [left, right] = screen.as_slice() {
        return app.draw_fs_spread_with_placement(
            ui,
            ctx,
            ui.max_rect(),
            left.idx,
            right.idx,
            false,
            FsPageTurnDecision::normal(),
            None,
            StillSeekGeometry::bar_only(),
            true,
            Some(&composition),
            selected_current,
            None,
        );
    }
    let material = app.resolve_fs_processed_material(ctx, idx, false);
    let payload = app.fullscreen_paint_payload_for_page(idx, material, false);
    let mut sources = FsNavigatorTextureSources::default();
    let bg = app.fs_bg_style(ctx);
    let transform = app
        .draw_fs_image_with_initial_placement(
            ui,
            ui.max_rect(),
            idx,
            app.fs_page_coordinate_source_size(idx),
            FsPageLayoutSource::CurrentItem,
            None,
            None,
            composition.singleton_placement(),
            payload.as_ref(),
            None,
            false,
            false,
            false,
            crate::rotation_db::Rotation::None,
            app.fs_zoom_pan(),
            0.0,
            &bg,
            "",
            false,
            app.effective_fullscreen_fit_mode(),
            app.fullscreen_fit_scale_limits(ctx.pixels_per_point()),
            None,
            Some(&mut sources),
            false,
            None,
            FlatCropPaint::WithPage,
            selected_current,
            false,
        )
        .unwrap();
    app.fullscreen_page_layout
        .begin(FullscreenPageLayoutKind::Single);
    app.fullscreen_page_layout
        .push_occurrence(transform, screen[0]);
    sources
}

fn fixture(
    mode: SpreadMode,
) -> (
    egui::Context,
    crate::app::AppTestEnvForTest,
    Vec<PreparedPagedPage>,
) {
    let (ctx, mut app, held) = start_native_turn(mode);
    prepare_absent_stage(&mut app, AbsentStage::CompletingOff);
    publish_quality(&mut app, &ctx, &held, [80, 120]);
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
    app.settings.margin_fit_enabled = false;
    app.settings.fullscreen_navigator_visible = false;
    app.settings.still_seek_strip_visible = false;
    app.fs_pan = egui::vec2(2400.0, -1800.0);
    // Seed an OFF completion after a reprojected frame. Use the real resolver
    // for its unbounded geometry, without an old binding choosing that geometry.
    let completion = std::mem::replace(
        &mut app.page_turn_presentation.state,
        PresentationState::Inactive,
    );
    let mut pages = Vec::new();
    frame(&ctx, |ui, ctx| {
        let sources = draw(&mut app, ui, ctx, false);
        let fit = app.effective_fullscreen_fit_mode();
        let limits = app.fullscreen_fit_scale_limits(ctx.pixels_per_point());
        let clip = if mode.is_spread() {
            canonical_demand::spread_clip_rect(
                ui.painter().clip_rect(),
                ui.max_rect(),
                app.fs_zoom_pan(),
                [None, None],
                fit,
                limits,
                false,
            )
        } else {
            canonical_demand::single_clip_rect(
                ui.painter().clip_rect(),
                ui.max_rect(),
                app.fs_zoom_pan(),
                0.0,
                None,
                fit,
                limits,
            )
        };
        let bg = app.fs_bg_style(ctx);
        for source in sources.pages {
            assert_eq!(
                source.resource.page_material_kind(),
                FullscreenPageMaterialKind::Complete
            );
            let transform = app
                .fullscreen_page_layout
                .page_by_idx(source.idx())
                .unwrap()
                .transform;
            pages.push(PreparedPagedPage {
                crop: None,
                occurrence: source.occurrence,
                transform,
                resource: source.resource,
                painter: ui.painter().with_clip_rect(clip),
                underlay: bg.clone(),
                live: false,
                spine: None,
            });
        }
    })
    .drop_without_applying_deltas();
    assert_ne!(
        draw_pan_correction(&pages.iter().map(|p| p.transform).collect::<Vec<_>>()),
        egui::Vec2::ZERO
    );
    let PresentationState::Completing {
        mut incoming,
        completion,
        continuation,
        diagnostics,
    } = completion
    else {
        panic!("real OFF completion");
    };
    let binding = app
        .page_turn_unit_binding(app.fullscreen_idx.unwrap())
        .unwrap()
        .with_prepared(&pages, None);
    let unit = Arc::make_mut(&mut incoming);
    // A published complete input must not move a frame whose final binding
    // still selects the immutable provisional Managed presentation.
    unit.pages = pages
        .iter()
        .map(|page| {
            let FullscreenPaintPayload::NormalResource(source) = &page.resource else {
                panic!("ordinary complete input");
            };
            assert!(source.source_id().is_some(), "ordinary resampleable input");
            PreparedPagedPage {
                resource: FullscreenPaintPayload::PresentationMaterial(
                    source
                        .clone()
                        .with_page_material_kind(FullscreenPageMaterialKind::Provisional)
                        .capture_presentation(&ctx),
                ),
                ..page.clone()
            }
        })
        .collect();
    unit.binding = binding;
    app.page_turn_presentation.state = PresentationState::Completing {
        incoming,
        completion,
        continuation,
        diagnostics,
    };
    assert!(
        pages
            .iter()
            .all(|page| app.page_turn_prepared_held_page(page).is_some())
    );
    (ctx, app, pages)
}

fn release_and_assert_first_complete_frame_bounds(app: &mut App, ctx: &egui::Context) {
    let pan = app.fs_pan;
    app.page_turn_presentation
        .deactivate("pan-bound-test-release");
    frame(ctx, |ui, ctx| {
        let sources = draw(app, ui, ctx, true);
        assert!(
            sources
                .pages
                .iter()
                .all(|source| source.resource.page_material_kind()
                    == FullscreenPageMaterialKind::Complete)
        );
        let transforms: Vec<_> = app
            .fullscreen_page_layout
            .displayed_pages()
            .map(|page| {
                assert_eq!(normal_zoom_pan(page.transform).unwrap().1, app.fs_pan);
                page.transform
            })
            .collect();
        assert_eq!(draw_pan_correction(&transforms), egui::Vec2::ZERO);
    })
    .drop_without_applying_deltas();
    assert_ne!(app.fs_pan, pan);
}

#[test]
fn held_single_matches_before_pan_and_bounds_the_first_released_complete_frame() {
    let (ctx, mut app, pages) = fixture(SpreadMode::Single);
    let pan = app.fs_pan;
    let mut corrected = pages[0].clone();
    let correction = draw_pan_correction(&[corrected.transform]);
    let t = corrected.transform;
    corrected.transform = resolve_fs_transform_in_layout_rect(
        DisplayedImageTransformInput {
            page_idx: t.page_idx,
            viewport_rect: t.viewport_rect,
            source_size: t.source_size,
            texture_size: t.texture_size,
            rotation: t.rotation,
            free_rotation_rad: t.free_rotation_rad,
            content_bbox: t.content_bbox,
            fit_mode: t.fit_input,
            fit_scale_limits: t.fit_scale_limits,
            pixels_per_point: t.pixels_per_point,
            placement: ResolvedDisplayPlacement::Normal {
                zoom_pan: Some((1.0, pan + correction)),
            },
            pixel_fit: t.pixel_fit,
        },
        t.layout_full_image_rect.translate(correction),
    )
    .unwrap();
    assert!(app.page_turn_prepared_held_page(&pages[0]).is_some());
    assert!(
        app.page_turn_prepared_held_page(&corrected).is_none(),
        "correcting first would change the binding choice"
    );
    frame(&ctx, |ui, ctx| {
        let sources = draw(&mut app, ui, ctx, true);
        assert!(
            sources
                .pages
                .iter()
                .all(|source| source.resource.page_material_kind()
                    == FullscreenPageMaterialKind::Provisional)
        );
        assert_eq!(app.fs_pan, pan);
        assert_eq!(
            normal_zoom_pan(app.fullscreen_page_layout.single_page().unwrap().transform)
                .unwrap()
                .1,
            pan
        );
    })
    .drop_without_applying_deltas();
    release_and_assert_first_complete_frame_bounds(&mut app, &ctx);
}

#[test]
fn held_spread_bounds_only_the_first_released_complete_frame() {
    let (ctx, mut app, _) = fixture(SpreadMode::Ltr);
    let pan = app.fs_pan;
    frame(&ctx, |ui, ctx| {
        let sources = draw(&mut app, ui, ctx, true);
        assert!(
            sources
                .pages
                .iter()
                .all(|source| source.resource.page_material_kind()
                    == FullscreenPageMaterialKind::Provisional)
        );
        assert_eq!(app.fs_pan, pan);
        assert_eq!(app.fullscreen_page_layout.displayed_pages().count(), 2);
    })
    .drop_without_applying_deltas();
    release_and_assert_first_complete_frame_bounds(&mut app, &ctx);
}

fn shrink_held_clip(app: &mut App, side: usize) {
    let PresentationState::Completing { incoming, .. } = &mut app.page_turn_presentation.state
    else {
        panic!("completion")
    };
    let unit = Arc::make_mut(incoming);
    let page = &mut unit.pages[side];
    page.painter = page
        .painter
        .with_clip_rect(page.painter.clip_rect().shrink(1.0));
}

#[test]
fn one_held_spread_page_keeps_the_entire_unit_pan() {
    for side in [0, 1] {
        let (ctx, mut app, pages) = fixture(SpreadMode::Ltr);
        shrink_held_clip(&mut app, side);
        assert!(app.page_turn_prepared_held_page(&pages[side]).is_none());
        assert!(app.page_turn_prepared_held_page(&pages[1 - side]).is_some());
        let pan = app.fs_pan;
        frame(&ctx, |ui, ctx| {
            let sources = draw(&mut app, ui, ctx, true);
            assert_eq!(
                sources
                    .pages
                    .iter()
                    .filter(|source| source.resource.page_material_kind()
                        == FullscreenPageMaterialKind::Provisional)
                    .count(),
                1
            );
            assert_eq!(app.fs_pan, pan, "held side {}", 1 - side);
        })
        .drop_without_applying_deltas();
    }
}

#[test]
fn completing_without_matching_held_pages_still_bounds_normal_material() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        let (ctx, mut app, pages) = fixture(mode);
        for side in 0..pages.len() {
            shrink_held_clip(&mut app, side);
        }
        assert!(app.page_turn_completion_active());
        assert!(
            pages
                .iter()
                .all(|page| app.page_turn_prepared_held_page(page).is_none())
        );
        let pan = app.fs_pan;
        frame(&ctx, |ui, ctx| {
            let sources = draw(&mut app, ui, ctx, true);
            assert!(
                sources
                    .pages
                    .iter()
                    .all(|source| source.resource.page_material_kind()
                        == FullscreenPageMaterialKind::Complete)
            );
        })
        .drop_without_applying_deltas();
        assert_ne!(app.fs_pan, pan, "the state alone must not exclude {mode:?}");
    }
}
