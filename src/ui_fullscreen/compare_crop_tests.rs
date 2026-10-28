//! Crop remains owned by the painted composite after comparison is ready.
use super::*;
use crate::export_crop::{CropAspectMode, CropRect, CropSettings};

fn assert_ready_compare_crop(editing: bool, oversized_legacy: bool) {
    for mode in [
        crate::app::CompareViewMode::PinnedNormal,
        crate::app::CompareViewMode::wipe(0.43),
        crate::app::CompareViewMode::Diff,
    ] {
        for zoom in [1.0, 1.7] {
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            page_turn::install(&ctx);
            install_fs_navigator_input_tracking(&ctx);
            let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
            #[cfg(windows)]
            {
                app.native_video_in_window_active = true;
            }
            app.settings.fullscreen_navigator_visible = false;
            app.settings.still_seek_strip_visible = false;
            app.settings.touch_still_chrome_learned = true;
            app.fs_zoom = zoom;
            app.fs_pan = egui::vec2(13.0, -7.0);
            app.export_crop_mode = editing;
            let mut crop = CropSettings::authored(
                CropRect {
                    min_x: 10.0,
                    min_y: 20.0,
                    max_x: 70.0,
                    max_y: 100.0,
                },
                CropAspectMode::Free,
                [80, 120],
            );
            if oversized_legacy {
                crop.source_size = None;
                crop.rect.max_x = 700.0;
            }
            app.export_crop_page_settings.insert(0, crop);
            let target_size = if oversized_legacy {
                [240, 360]
            } else {
                [80, 120]
            };
            let pixels = Arc::new(egui::ColorImage::filled(
                target_size,
                egui::Color32::LIGHT_BLUE,
            ));
            app.pinned_compare_slot = Some(crate::app::PinnedCompareSlot {
                pixels: pixels.clone(),
                indicator_pixels: pixels.clone(),
                indicator_texture: None,
                display_name: "pinned".into(),
                source_idx: 2,
                external_item_key: None,
                source_size: target_size,
            });
            app.compare_view_mode = mode;
            let prepared = match mode {
                crate::app::CompareViewMode::PinnedNormal => {
                    crate::app::ComparePreparedPixels::PinnedTexture(pixels)
                }
                crate::app::CompareViewMode::Wipe { .. } => {
                    crate::app::ComparePreparedPixels::WipeTextures {
                        pinned: pixels.clone(),
                        current: pixels,
                    }
                }
                crate::app::CompareViewMode::Diff => {
                    crate::app::ComparePreparedPixels::DiffTexture(pixels)
                }
                _ => unreachable!(),
            };
            app.compare_preparation =
                crate::app::ComparePreparationState::Ready(crate::app::ComparePreparedPair {
                    key: 81,
                    current_idx: 0,
                    pinned_source_idx: 2,
                    current_input_size: [80, 120],
                    pinned_input_size: target_size,
                    target_size,
                    pixels: prepared,
                    pinned_texture: None,
                    current_texture: None,
                    diff_texture: None,
                });
            // Enter the actual fullscreen dispatch, after comparison preparation.
            // Warm-up passes settle chrome; each output must contain exactly one crop.
            for frame in 1..=3 {
                app.frame_counter += 1;
                let output = crate::test_ui::headless_paint_output(ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(960.0, 720.0),
                        )),
                        time: Some(frame as f64 / 60.0),
                        focused: true,
                        ..Default::default()
                    },
                    |ui| app.render_fullscreen_viewport(ui),
                ));
                assert!(
                    app.compare_prepared_pair_matches(0),
                    "comparison must stay ready"
                );
                assert_eq!(app.compare_view_mode, mode, "crop must not exit comparison");
                let pair = app.compare_preparation.prepared_pair().unwrap();
                let compare_textures: Vec<_> = match mode {
                    crate::app::CompareViewMode::PinnedNormal => {
                        vec![pair.pinned_texture.as_ref().unwrap().id()]
                    }
                    crate::app::CompareViewMode::Wipe { .. } => vec![
                        pair.current_texture.as_ref().unwrap().id(),
                        pair.pinned_texture.as_ref().unwrap().id(),
                    ],
                    crate::app::CompareViewMode::Diff => {
                        vec![pair.diff_texture.as_ref().unwrap().id()]
                    }
                    _ => unreachable!(),
                };
                let painted_textures: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Mesh(mesh) => Some(mesh.texture_id),
                        _ => None,
                    })
                    .collect();
                for texture in compare_textures {
                    assert!(
                        painted_textures.contains(&texture),
                        "the prepared comparison itself must be painted"
                    );
                }
                assert!(
                    !painted_textures
                        .contains(&app.source_material_for_idx(0).unwrap().source_texture_id()),
                    "ready comparison must not silently fall back to ordinary page paint"
                );
                let transform = app
                    .fullscreen_page_layout
                    .single_page()
                    .expect("comparison layout")
                    .transform;
                let projected = crate::page_turn_animation::PageCropPaint::from_screen_corners(
                    crate::ui_crop::crop_corners(&transform, [80, 120], CropRect::full(80, 120)),
                    crate::ui_crop::crop_corners(
                        &transform,
                        [80, 120],
                        crop.rect.sanitized(80, 120),
                    ),
                    transform.paint_rect,
                );
                let expected = crate::page_turn_animation::project_flat_crop(
                    transform.paint_rect,
                    transform.viewport_rect,
                    &transform.paint_quad().positions,
                    projected,
                );
                // These two colors uniquely identify crop dimming and its inner border.
                // Compare every vertex (and multiplicity), so absence and double paint both fail.
                let crop_vertices = |meshes: Vec<egui::Mesh>| {
                    meshes
                        .into_iter()
                        .flat_map(|mesh| mesh.vertices)
                        .filter(|v| {
                            v.color == egui::Color32::from_black_alpha(145)
                                || v.color == egui::Color32::from_rgb(255, 230, 100)
                        })
                        .map(|v| (v.pos, v.color))
                        .collect::<Vec<_>>()
                };
                let actual = crop_vertices(
                    output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::Shape::Mesh(mesh) => Some((**mesh).clone()),
                            _ => None,
                        })
                        .collect(),
                );
                let expected = crop_vertices(expected.into_iter().map(|draw| draw.mesh).collect());
                assert!(!expected.is_empty());
                assert_eq!(
                    actual.len(),
                    expected.len(),
                    "mode={mode:?} edit={editing} zoom={zoom} frame={frame}"
                );
                for ((pos, color), (expected_pos, expected_color)) in actual.iter().zip(&expected) {
                    assert_eq!(color, expected_color);
                    assert!(
                        (*pos - *expected_pos).length() < 0.001,
                        "crop follows the actual compare transform"
                    );
                }
                let handles = output.shapes.iter().filter(|shape| matches!(
                    &shape.shape, egui::Shape::Circle(circle) if circle.fill == egui::Color32::from_rgb(255, 245, 180)
                )).count();
                assert_eq!(handles, if editing { 8 } else { 0 });
                if editing {
                    let expected_handles = crate::ui_crop::crop_corners(
                        &transform,
                        [80, 120],
                        crop.rect.sanitized(80, 120),
                    );
                    let bounds = transform.viewport_rect.shrink(14.0);
                    for expected in expected_handles {
                        let expected = egui::pos2(
                            expected.x.clamp(bounds.left(), bounds.right()),
                            expected.y.clamp(bounds.top(), bounds.bottom()),
                        );
                        assert!(
                            output.shapes.iter().any(|shape| matches!(
                                &shape.shape, egui::Shape::Circle(circle)
                                    if circle.fill == egui::Color32::from_rgb(255, 245, 180)
                                        && (circle.center - expected).length() < 0.001
                            )),
                            "crop corner handles must follow the painted frame"
                        );
                    }
                }
                assert_eq!(
                    app.export_crop_page_settings[&0], crop,
                    "comparison paint must not rewrite unresolved document crop"
                );
            }
        }
    }
}

#[test]
fn crop_ready_compare_browsing_paints_dim_and_border_once() {
    assert_ready_compare_crop(false, false);
}

#[test]
fn crop_ready_compare_editing_paints_dim_and_border_once_with_handles() {
    assert_ready_compare_crop(true, false);
}

#[test]
fn crop_ready_compare_oversized_legacy_uses_source_coordinates_for_frame_and_handles() {
    assert_ready_compare_crop(false, true);
    assert_ready_compare_crop(true, true);
}
