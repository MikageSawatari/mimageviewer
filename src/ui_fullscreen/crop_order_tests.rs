//! Actual fullscreen paint order: grid/canvas/Z precede the single shared crop owner.
#![cfg(windows)]
use super::*;
use crate::export_crop::{CropAspectMode, CropRect, CropSettings};

#[derive(Clone, Copy, Debug)]
enum View {
    Normal,
    Grid,
    Text,
    Aim,
    Expanded,
}

fn check_crop_order(view: View, editing: bool) {
    let _input = crate::key_input::lock_test_input();
    for animation in [false, true] {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        page_turn::install(&ctx);
        install_fs_navigator_input_tracking(&ctx);
        let mut app = page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
        app.native_video_in_window_active = true;
        app.fs_prev_focused = true;
        app.settings.page_turn_animation_enabled = animation;
        app.settings.fullscreen_navigator_visible = false;
        app.settings.still_seek_strip_visible = false;
        app.settings.touch_still_chrome_learned = true;
        app.export_crop_mode = editing;
        app.fs_pixel_grid_enabled = true;
        app.fs_zoom_aiming = matches!(view, View::Aim);
        app.fs_zoom_active = matches!(view, View::Expanded);
        app.fs_zoom_z_was_down = matches!(view, View::Aim);
        app.fs_zoom_factor = 2.0;
        if matches!(view, View::Grid) {
            app.fs_zoom = 3.0;
        }
        if matches!(view, View::Text) {
            app.text_mode = true;
            app.comic_font_registry_loaded = true;
            app.comic_fonts.seed_for_test(None, Default::default());
            app.comic_presets_loaded = true;
            app.text_smart_guides.vertical = Some(35.0);
            app.text_smart_guides.horizontal = Some(50.0);
            app.text_marquee = Some(crate::app::TextMarquee {
                start_screen: egui::pos2(330.0, 220.0),
                current_screen: egui::pos2(500.0, 450.0),
                additive: false,
            });
        }
        let mut keys = crate::keymap::KeymapSettings::default();
        keys.set_override_chords(
            KeyAction::FsZoomMode,
            vec![Chord::key(KeyName::NumpadEnter)],
        );
        app.keymap = Keymap::from_settings(&keys);
        let crop = CropSettings::authored(
            CropRect {
                min_x: 10.0,
                min_y: 20.0,
                max_x: 70.0,
                max_y: 100.0,
            },
            CropAspectMode::Free,
            [80, 120],
        );
        app.export_crop_page_settings.insert(0, crop);
        for frame in 1..=2 {
            crate::key_input::set_test_frame_for_viewport(ctx.viewport_id(), vec![]);
            crate::key_input::set_test_return_key_state(
                ctx.viewport_id(),
                false,
                matches!(view, View::Aim),
            );
            app.frame_counter += 1;
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 720.0),
                )),
                time: Some(frame as f64 / 60.0),
                focused: true,
                ..Default::default()
            };
            input
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .focused = Some(true);
            let output = crate::test_ui::headless_paint_output(
                ctx.run_ui(input, |ui| app.render_fullscreen_viewport(ui)),
            );
            assert_eq!(
                app.fs_zoom_aiming,
                matches!(view, View::Aim),
                "fixture must reach the actual requested Z branch"
            );
            assert_eq!(app.fs_zoom_active, matches!(view, View::Expanded));
            let transform = app.fullscreen_page_layout.single_page().unwrap().transform;
            assert_eq!(
                matches!(transform.placement, ResolvedDisplayPlacement::Z { .. }),
                matches!(view, View::Aim | View::Expanded)
            );
            // Ordinary preparation may publish a final texture with different
            // sampler options from this fixture's raw texture. Check the actual
            // selected display material, rather than assuming handle sharing.
            let source = app
                .resolve_fs_display_material(0, true)
                .unwrap()
                .source_texture_id();
            let image = output
                .shapes
                .iter()
                .position(|s| {
                    matches!(
                        &s.shape, egui::Shape::Mesh(mesh) if mesh.texture_id == source
                    )
                })
                .expect("actual image mesh");
            let crops: Vec<_> = output
                .shapes
                .iter()
                .enumerate()
                .filter_map(|(i, s)| match &s.shape {
                    egui::Shape::Mesh(mesh)
                        if mesh
                            .vertices
                            .iter()
                            .any(|v| v.color == egui::Color32::from_rgb(255, 230, 100)) =>
                    {
                        Some((i, mesh))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                crops.len(),
                1,
                "one crop owner in {view:?}, animation={animation}, editing={editing}"
            );
            let (crop_order, mesh) = crops[0];
            assert!(image < crop_order);
            let dim = mesh
                .vertices
                .iter()
                .position(|v| v.color == egui::Color32::from_black_alpha(145))
                .unwrap();
            let border = mesh
                .vertices
                .iter()
                .position(|v| v.color == egui::Color32::from_rgb(255, 230, 100))
                .unwrap();
            assert!(dim < border, "crop dim must precede its black/yellow frame");
            // Exact common-owner geometry and vertex multiplicity catch both omission
            // and a second crop drawn by the canvas overlay or Z helper.
            let expected = crate::page_turn_animation::project_flat_crop(
                transform.paint_rect,
                transform.viewport_rect,
                &transform.paint_quad().positions,
                crate::page_turn_animation::PageCropPaint::from_screen_corners(
                    crate::ui_crop::crop_corners(&transform, [80, 120], CropRect::full(80, 120)),
                    crate::ui_crop::crop_corners(&transform, [80, 120], crop.rect),
                    transform.paint_rect,
                ),
            );
            assert_eq!(expected.len(), 1);
            assert_eq!(mesh.vertices, expected[0].mesh.vertices);
            let aim: Vec<_> = output
                .shapes
                .iter()
                .enumerate()
                .filter_map(|(i, s)| match &s.shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == egui::Color32::from_black_alpha(120) =>
                    {
                        Some(i)
                    }
                    _ => None,
                })
                .collect();
            if matches!(view, View::Aim) {
                let last_dim = *aim.last().expect("actual Z aim dimming must be present");
                assert!(image < aim[0]);
                let aim_frame = last_dim + 1;
                assert!(
                    matches!(&output.shapes[aim_frame].shape, egui::Shape::Rect(rect) if rect.stroke.width == 2.0)
                );
                assert!(
                    aim_frame < crop_order,
                    "aim dim/frame must precede crop dim/frame"
                );
            } else {
                assert!(aim.is_empty(), "ordinary/expanded view has no aim dimming");
            }
            let grid: Vec<_> = output
                .shapes
                .iter()
                .enumerate()
                .filter_map(|(i, s)| match &s.shape {
                    egui::Shape::LineSegment { stroke, .. }
                        if stroke.color == egui::Color32::from_black_alpha(120) =>
                    {
                        Some(i)
                    }
                    _ => None,
                })
                .collect();
            if matches!(view, View::Expanded | View::Grid) {
                assert!(image < grid[0], "actual zoomed pixel grid");
                assert!(
                    *grid.last().unwrap() < crop_order,
                    "normal/expanded Z grid must remain below crop"
                );
            }
            if matches!(view, View::Text) {
                let guides: Vec<_> = output
                    .shapes
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| {
                        matches!(&s.shape, egui::Shape::LineSegment { stroke, .. }
                        if stroke.color == egui::Color32::from_rgb(255, 80, 210))
                        .then_some(i)
                    })
                    .collect();
                assert_eq!(guides.len(), 2, "actual text canvas guides");
                let selection = output.shapes.iter().enumerate().find_map(|(i, s)| {
                    matches!(&s.shape, egui::Shape::Rect(rect)
                        if rect.stroke.width == 1.2 && rect.stroke.color == egui::Color32::from_rgb(80, 180, 255)).then_some(i)
                }).expect("actual canvas marquee selection");
                assert!(
                    guides
                        .iter()
                        .chain(std::iter::once(&selection))
                        .all(|i| image < *i && *i < crop_order),
                    "canvas selection/guides must precede crop dim and frame"
                );
            }
            let handles: Vec<_> = output
                .shapes
                .iter()
                .enumerate()
                .filter_map(|(i, s)| match &s.shape {
                    egui::Shape::Circle(c) if c.fill == egui::Color32::from_rgb(255, 245, 180) => {
                        Some(i)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(handles.len(), if editing { 8 } else { 0 });
            assert!(handles.iter().all(|i| *i > crop_order));
        }
    }
}

#[test]
fn crop_z_aim_frame_and_dim_paint_after_aim_overlay_once() {
    check_crop_order(View::Aim, false);
}

#[test]
fn crop_z_expanded_frame_and_dim_paint_after_pixel_grid_once() {
    check_crop_order(View::Expanded, false);
}

#[test]
fn crop_normal_frame_order_and_edit_handles_stay_unchanged() {
    check_crop_order(View::Normal, false);
    check_crop_order(View::Normal, true);
}

#[test]
fn crop_normal_pixel_grid_precedes_dim_frame_and_edit_handles() {
    check_crop_order(View::Grid, false);
    check_crop_order(View::Grid, true);
}

#[test]
fn crop_normal_text_canvas_selection_and_guides_precede_dim_frame() {
    check_crop_order(View::Text, false);
}
