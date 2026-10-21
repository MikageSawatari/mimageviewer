use super::*;
use crate::page_dims::{CanonicalPageDimensions, PagePixelDimensions};
use crate::source_coverage::{EditPreviewGeometry, SourceCoverage};

fn draw_spread(app: &mut App, ctx: &egui::Context) -> [DisplayedImageTransform; 2] {
    draw_spread_using(app, ctx, None)
}

fn draw_spread_using(
    app: &mut App,
    ctx: &egui::Context,
    held: Option<&FsDisplayUnitHoldover>,
) -> [DisplayedImageTransform; 2] {
    let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 950.0));
    let _ = ctx
        .run_pass(
            egui::RawInput {
                screen_rect: Some(viewport),
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|root| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(root, |ui| {
                            let display_override =
                                held.map(|unit| (&unit.pages[0], &unit.pages[1]));
                            app.draw_fs_spread(
                                ui,
                                ctx,
                                viewport,
                                0,
                                1,
                                false,
                                FsPageTurnDecision::normal(),
                                display_override,
                            );
                        });
                })
            },
        )
        .drop_without_applying_deltas();
    std::array::from_fn(|idx| {
        app.fullscreen_page_layout
            .page_by_idx(idx)
            .expect("spread page submitted")
            .transform
    })
}

fn install_original(app: &mut App, ctx: &egui::Context, idx: usize) {
    let pixels = Arc::new(egui::ColorImage::filled([600, 800], egui::Color32::WHITE));
    let tex = ctx.load_texture(
        format!("original-{idx}"),
        (*pixels).clone(),
        egui::TextureOptions::LINEAR,
    );
    app.fs_cache.insert(
        idx,
        FsCacheEntry::Static {
            tex,
            pixels,
            source_dims: Some([600, 800]),
            load_seq: 0,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
}

#[test]
fn cropped_spread_draw_entry_keeps_layout_in_both_arrival_orders_and_all_fits() {
    for ppp in [1.0, 1.25, 1.5] {
        for crop_idx in [0, 1] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(ppp);
            let mut app = crate::app::setup_app_for_test();
            app.items = (0..2)
                .map(|idx| GridItem::Image(app.tmp.path().join(format!("spread-{idx}.png"))))
                .collect();
            app.fullscreen_idx = Some(0);
            app.visible_indices = vec![0, 1];
            app.details_order = vec![0, 1];
            app.spread_mode = SpreadMode::Ltr;
            app.settings.spread_page_gap_px = 2;
            let coverage = SourceCoverage::Known(
                EditPreviewGeometry::new(
                    CanonicalPageDimensions::new(600, 800),
                    (600, 800),
                    (200, 100, 200, 600),
                )
                .unwrap(),
            );
            app.thumbnails = (0..2)
                .map(|idx| {
                    let cropped = idx == crop_idx;
                    let size = if cropped { [33, 100] } else { [100, 133] };
                    ThumbnailState::Loaded {
                        tex: ctx.load_texture(
                            format!("thumbnail-{idx}"),
                            egui::ColorImage::filled(size, egui::Color32::WHITE),
                            egui::TextureOptions::LINEAR,
                        ),
                        origin: crate::thumb_loader::ThumbLoadOrigin::EditPreviewCache { epoch: 0 },
                        from_edit_preview: cropped,
                        source_coverage: if cropped {
                            coverage
                        } else {
                            SourceCoverage::FullSource
                        },
                        rendered_at_px: 133,
                        source_dims: Some(if cropped { (200, 600) } else { (600, 800) }),
                        layout_dims: None,
                    }
                })
                .collect();
            for idx in 0..2 {
                let generation = app.items_generation;
                app.page_dims_cache.record(
                    generation,
                    idx,
                    PagePixelDimensions::Canonical(CanonicalPageDimensions::new(600, 800)),
                );
            }
            for mode in [
                FullscreenFitMode::Page,
                FullscreenFitMode::Width,
                FullscreenFitMode::Height,
                FullscreenFitMode::Original,
                FullscreenFitMode::SpecifiedScale,
            ] {
                app.settings.fullscreen_fit_mode = mode;
                for order in [[0, 1], [1, 0]] {
                    app.fs_cache.clear();
                    app.fs_early_dims.clear();
                    let before = draw_spread(&mut app, &ctx);
                    let held = app
                        .capture_fs_display_unit_with_rendition_for_pair(
                            None,
                            0,
                            SpreadPair::Double { left: 0, right: 1 },
                        )
                        .expect("capture both thumbnail resources");
                    let mut transitions = vec![before];
                    for idx in order {
                        app.fs_early_dims.insert(idx, [600, 800]);
                        transitions.push(draw_spread(&mut app, &ctx));
                        install_original(&mut app, &ctx, idx);
                        transitions.push(draw_spread(&mut app, &ctx));
                    }
                    let originals = *transitions.last().unwrap();
                    transitions.push(draw_spread_using(&mut app, &ctx, Some(&held)));
                    app.fs_cache.clear();
                    app.fs_early_dims.clear();
                    transitions.push(draw_spread(&mut app, &ctx));
                    for state in transitions {
                        let actual = state[crop_idx];
                        let expected = originals[crop_idx];
                        let max_delta = (actual.full_image_rect.min - expected.full_image_rect.min)
                            .length()
                            .max(
                                (actual.full_image_rect.max - expected.full_image_rect.max)
                                    .length(),
                            )
                            * ppp;
                        assert!(
                            max_delta <= 1.01,
                            "mode={mode:?} crop={crop_idx} order={order:?} ppp={ppp}: {actual:?} != {expected:?}"
                        );
                        let common_pixel = egui::pos2(0.4, 0.5);
                        assert!(
                            (actual.source_normalized_to_screen(common_pixel)
                                - expected.source_normalized_to_screen(common_pixel))
                            .length()
                                * ppp
                                <= 1.01
                        );
                        // The unedited side still uses its ordinary thumbnail. Its
                        // independently rounded aspect may contain inside the frame;
                        // it must never set the magnitude of the composed spread.
                        let sibling = state[1 - crop_idx];
                        assert!(
                            (sibling.full_image_rect.center().x
                                - originals[1 - crop_idx].full_image_rect.center().x)
                                .abs()
                                * ppp
                                <= 1.01
                        );
                        if mode == FullscreenFitMode::Original {
                            assert!((actual.full_image_rect.width() * ppp - 600.0).abs() <= 1.01);
                            assert!((sibling.full_image_rect.width() * ppp - 600.0).abs() <= 1.01);
                        }
                    }
                }
            }
        }
    }
}
