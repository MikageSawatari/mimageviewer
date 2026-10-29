//! Headless production handlers: key dispatch → pipeline sync → canvas → panel.
use super::*;
use crate::conceal::ConcealTool;
use crate::displayed_image_transform::{
    DisplayedImageTransformInput, FullscreenFitScaleLimits, RectPixelFit, ResolvedDisplayPlacement,
};
use crate::edit_preview::EditPreviewHoldState;
use crate::key_input::{SyntheticKeyCommand, SyntheticNavigationKey};

fn key_level(key: SyntheticNavigationKey, down: bool) {
    let now = std::time::Instant::now();
    crate::key_input::enqueue_test_synthetic_command(if down {
        SyntheticKeyCommand::down(now, key, Default::default())
    } else {
        SyntheticKeyCommand::up(now, key)
    });
    crate::key_input::advance_test_synthetic_input(now);
}

fn fixture() -> (crate::app::AppTestEnvForTest, egui::Context) {
    crate::key_input::arm_test_synthetic_input(0xE040, egui::ViewportId::ROOT);
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    crate::egui_focus_policy::install_tab_shortcut_focus_policy(&ctx);
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![crate::grid_item::GridItem::Image(
        app.tmp.path().join("conceal-preview.png"),
    )];
    app.fullscreen_idx = Some(0);
    app.conceal_mode = true;
    app.conceal_tool = ConcealTool::Brush;
    app.conceal_mask_size = [200, 200];
    app.conceal_mask = Some(vec![false; 200 * 200]);
    app.settings.conceal_brush_radius = 1.0;
    let pixels = Arc::new(egui::ColorImage::filled([200, 200], egui::Color32::WHITE));
    app.conceal_base_cache.insert(0, Arc::clone(&pixels));
    let generation = app.conceal_generation;
    app.conceal_cache.insert(
        0,
        crate::app::ConcealCacheEntry {
            texture_options: egui::TextureOptions::LINEAR,
            display_basis: crate::material_display_basis::MaterialDisplayBasis::from_sampler(
                [200, 200],
                crate::source_coverage::SourceCoverage::FullSource,
            ),
            pixels,
            texture: None,
            generation,
        },
    );
    (app, ctx)
}

fn frame(
    app: &mut App,
    ctx: &egui::Context,
    key: egui::Key,
    pointer: Option<(egui::Pos2, Option<bool>)>,
    owner: Option<crate::keyboard_input::KeyboardOwner>,
) -> bool {
    frame_with_extra_events(app, ctx, key, pointer, owner, &[])
}

fn frame_with_extra_events(
    app: &mut App,
    ctx: &egui::Context,
    key: egui::Key,
    pointer: Option<(egui::Pos2, Option<bool>)>,
    owner: Option<crate::keyboard_input::KeyboardOwner>,
    extra_events: &[egui::Event],
) -> bool {
    // Keep a repeat event after release too: the OS level must beat stale egui state.
    let mut events = vec![egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: true,
        modifiers: egui::Modifiers::NONE,
    }];
    events.extend_from_slice(extra_events);
    if let Some((pos, edge)) = pointer {
        events.push(egui::Event::PointerMoved(pos));
        if let Some(pressed) = edge {
            events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
    }
    let full_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));
    let mut pipeline_active = false;
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(full_rect),
            focused: true,
            events,
            ..Default::default()
        },
        |_ui| {
            app.keyboard_owner_for_pass(ctx);
            if let Some(owner) = owner {
                crate::keyboard_input::cache_keyboard_owner(ctx, owner);
            }
            if !crate::keyboard_input::keymap_owner_blocks_shortcuts(ctx)
                && owner != Some(crate::keyboard_input::KeyboardOwner::Modal)
            {
                if app.erase_mode {
                    app.handle_erase_keys(ctx, 0);
                } else {
                    app.handle_conceal_keys(ctx, 0);
                }
                if extra_events.is_empty() {
                    assert!(!ctx.input(|i| i.events.iter().any(|event| matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::Tab,
                            ..
                        }
                    ))));
                }
            }
            app.sync_conceal_preview_before_pipeline(ctx, egui::ViewportId::ROOT);
            pipeline_active = app.conceal_preview_active;
            let transform = DisplayedImageTransform::from_resolved_rect(
                DisplayedImageTransformInput {
                    page_idx: 0,
                    viewport_rect: full_rect,
                    source_size: egui::vec2(200.0, 200.0),
                    texture_size: egui::vec2(200.0, 200.0),
                    rotation: crate::rotation_db::Rotation::None,
                    free_rotation_rad: 0.0,
                    content_bbox: None,
                    fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
                        crate::settings::FullscreenFitMode::Page,
                    ),
                    fit_scale_limits: FullscreenFitScaleLimits::default(),
                    pixels_per_point: 1.0,
                    placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
                    pixel_fit: RectPixelFit::Texels,
                },
                egui::Rect::from_min_size(egui::pos2(600.0, 100.0), egui::vec2(200.0, 200.0)),
            )
            .unwrap();
            // Same suppression retirement as handle_fs_wheel_and_click, before canvas dispatch.
            let _ =
                app.fs_primary_suppression
                    .apply(FullscreenPrimarySuppressionEvent::PointerFrame {
                        released: ctx.input(|i| i.pointer.primary_released()),
                    });
            if app.erase_mode {
                app.handle_erase_paint(ctx, &transform, Default::default());
                app.draw_erase_overlay(_ui, ctx, &transform);
            } else {
                app.handle_conceal_paint(ctx, &transform, Default::default());
                app.draw_conceal_panel(ctx, full_rect);
            }
        },
    )
    .drop_without_applying_deltas();
    pipeline_active
}

#[test]
fn conceal_preview_hold_release_and_eye_share_preview_without_invalidating_cache() {
    let _input = crate::key_input::lock_test_input();
    let (mut app, ctx) = fixture();
    let pixels = Arc::clone(&app.conceal_cache[&0].pixels);
    let generation = app.conceal_generation;
    key_level(SyntheticNavigationKey::Tab, true);
    frame(&mut app, &ctx, egui::Key::Tab, None, None);
    assert!(app.conceal_preview_active);
    assert!(frame(&mut app, &ctx, egui::Key::Tab, None, None));
    let eye = app.conceal_preview_button_rect.unwrap().center();
    frame(
        &mut app,
        &ctx,
        egui::Key::Tab,
        Some((eye, Some(true))),
        None,
    );
    key_level(SyntheticNavigationKey::Tab, false);
    frame(&mut app, &ctx, egui::Key::Tab, Some((eye, None)), None);
    assert!(app.conceal_preview_active, "eye holds after key release");
    frame(
        &mut app,
        &ctx,
        egui::Key::Tab,
        Some((eye, Some(false))),
        None,
    );
    assert!(!app.conceal_preview_active);
    assert!(!frame(&mut app, &ctx, egui::Key::Tab, None, None));
    assert_eq!(app.conceal_generation, generation);
    assert!(Arc::ptr_eq(&pixels, &app.conceal_cache[&0].pixels));
    key_level(SyntheticNavigationKey::Tab, true);
    frame(&mut app, &ctx, egui::Key::Tab, None, None);
    app.reset_conceal_mode();
    assert_eq!(app.conceal_preview_hold, EditPreviewHoldState::Released);
    assert!(!app.conceal_preview_active);
}

#[test]
fn conceal_preview_hold_during_drawing_and_on_release_requires_a_new_press() {
    let _input = crate::key_input::lock_test_input();
    for kind in 0..5 {
        let (mut app, ctx) = fixture();
        match kind {
            0 => app.conceal_last_paint_pos = Some(egui::pos2(700.0, 200.0)),
            1 => app.conceal_line_start = Some((0.0, 0.0)),
            2 => app.conceal_shape_drag_start = Some((0.0, 0.0)),
            3 => app.conceal_lasso_points.push((0.0, 0.0)),
            _ => {
                app.conceal_drag = Some(crate::vector_edit::begin_drag(
                    crate::vector_edit::HoverTarget::Body,
                    0,
                    crate::mask_db::Shape::Rect {
                        center: (10.0, 10.0),
                        half_w: 5.0,
                        half_h: 5.0,
                        rotation_rad: 0.0,
                        op: crate::mask_db::ShapeOp::Add,
                    },
                    (0.0, 0.0),
                ))
            }
        }
        key_level(SyntheticNavigationKey::Tab, true);
        frame(&mut app, &ctx, egui::Key::Tab, None, None);
        assert_eq!(
            app.conceal_preview_hold,
            EditPreviewHoldState::BlockedUntilRelease
        );
        assert!(!app.conceal_preview_active, "kind={kind}");
        app.conceal_last_paint_pos = None;
        app.conceal_line_start = None;
        app.conceal_shape_drag_start = None;
        app.conceal_lasso_points.clear();
        app.conceal_drag = None;
        frame(&mut app, &ctx, egui::Key::Tab, None, None);
        assert!(!app.conceal_preview_active);
        key_level(SyntheticNavigationKey::Tab, false);
        frame(&mut app, &ctx, egui::Key::Tab, None, None);
        key_level(SyntheticNavigationKey::Tab, true);
        frame(&mut app, &ctx, egui::Key::Tab, None, None);
        assert!(app.conceal_preview_active);
    }
}

#[test]
fn conceal_preview_hold_blocks_brush_selection_and_shape_operations_until_pointer_release() {
    let _input = crate::key_input::lock_test_input();
    for tool in [
        ConcealTool::Brush,
        ConcealTool::Select,
        ConcealTool::Rect,
        ConcealTool::Lasso,
        ConcealTool::Polygon,
        ConcealTool::Bucket,
    ] {
        let (mut app, ctx) = fixture();
        app.conceal_tool = tool;
        if tool == ConcealTool::Select {
            app.conceal_shapes.push(crate::mask_db::Shape::Rect {
                center: (100.0, 100.0),
                half_w: 30.0,
                half_h: 30.0,
                rotation_rad: 0.0,
                op: crate::mask_db::ShapeOp::Add,
            });
        }
        let shapes = app.conceal_shapes.clone();
        key_level(SyntheticNavigationKey::Tab, true);
        frame(&mut app, &ctx, egui::Key::Tab, None, None);
        let pos = egui::pos2(700.0, 200.0);
        frame(
            &mut app,
            &ctx,
            egui::Key::Tab,
            Some((pos, Some(true))),
            None,
        );
        assert!(app.fs_primary_suppression.is_active());
        key_level(SyntheticNavigationKey::Tab, false);
        frame(
            &mut app,
            &ctx,
            egui::Key::Tab,
            Some((pos + egui::vec2(20.0, 0.0), None)),
            None,
        );
        frame(&mut app, &ctx, egui::Key::Tab, Some((pos, None)), None);
        assert!(!app.conceal_preview_active);
        assert!(app.conceal_last_paint_pos.is_none(), "{tool:?}");
        assert!(app.conceal_line_start.is_none());
        assert!(app.conceal_shape_drag_start.is_none());
        assert!(app.conceal_drag.is_none());
        assert!(app.conceal_lasso_points.is_empty());
        assert!(!app.conceal_mask.as_ref().unwrap().iter().any(|p| *p));
        assert_eq!(app.conceal_shapes, shapes);
        assert!(app.conceal_selected_shape.is_none());
        frame(
            &mut app,
            &ctx,
            egui::Key::Tab,
            Some((pos, Some(false))),
            None,
        );
        app.conceal_tool = ConcealTool::Brush;
        frame(
            &mut app,
            &ctx,
            egui::Key::Tab,
            Some((pos, Some(true))),
            None,
        );
        assert!(app.conceal_mask.as_ref().unwrap().iter().any(|p| *p));
    }
}

#[test]
fn conceal_preview_hold_simultaneous_brush_start_and_brush_release_keep_drawing_owner() {
    let _input = crate::key_input::lock_test_input();
    for on_release in [false, true] {
        let (mut app, ctx) = fixture();
        let pos = egui::pos2(700.0, 200.0);
        if on_release {
            frame(
                &mut app,
                &ctx,
                egui::Key::Tab,
                Some((pos, Some(true))),
                None,
            );
            assert!(app.conceal_last_paint_pos.is_some());
        }
        key_level(SyntheticNavigationKey::Tab, true);
        frame(
            &mut app,
            &ctx,
            egui::Key::Tab,
            Some((pos, Some(!on_release))),
            None,
        );
        assert_eq!(
            app.conceal_preview_hold,
            EditPreviewHoldState::BlockedUntilRelease
        );
        assert!(!app.conceal_preview_active);
        if !on_release {
            frame(
                &mut app,
                &ctx,
                egui::Key::Tab,
                Some((pos, Some(false))),
                None,
            );
        }
        assert!(app.conceal_last_paint_pos.is_none());
        frame(&mut app, &ctx, egui::Key::Tab, None, None);
        assert!(!app.conceal_preview_active);
    }
}

#[test]
fn conceal_preview_hold_rebinding_disable_and_keyboard_ownership() {
    use crate::keyboard_input::{KeyboardOwner, TextInputPhase};
    let _input = crate::key_input::lock_test_input();
    let (mut app, ctx) = fixture();
    // G is supported by synthetic input. Free its ordinary operation for this binding test.
    app.keymap = crate::keymap::Keymap::from_ini_str(
        "[Conceal]\nConcealPreviewHold = G\nConcealPixelGrid = none\n",
    );
    key_level(SyntheticNavigationKey::Tab, true);
    frame(&mut app, &ctx, egui::Key::Tab, None, None);
    assert!(!app.conceal_preview_active);
    key_level(SyntheticNavigationKey::Tab, false);
    key_level(SyntheticNavigationKey::G, true);
    for owner in [
        KeyboardOwner::Modal,
        KeyboardOwner::TextInput {
            viewport: egui::ViewportId::ROOT,
            widget_id: egui::Id::unique("conceal-ime"),
            phase: TextInputPhase::ImeComposing,
        },
    ] {
        frame(&mut app, &ctx, egui::Key::G, None, Some(owner));
        assert!(!app.conceal_preview_active);
    }
    frame(&mut app, &ctx, egui::Key::G, None, None);
    assert!(app.conceal_preview_active);
    app.keymap = crate::keymap::Keymap::from_ini_str(
        "[Conceal]\nConcealPreviewHold = none\nConcealPixelGrid = none\n",
    );
    frame(&mut app, &ctx, egui::Key::G, None, None);
    assert!(!app.conceal_preview_active);
}

fn set_erase_mode(app: &mut App) {
    app.conceal_mode = false;
    app.erase_mode = true;
    app.erase_tool = crate::app::EraseTool::Brush;
    app.erase_mask_size = [200, 200];
    app.erase_mask = Some(vec![false; 200 * 200]);
    app.erase_base_cache.insert(
        0,
        crate::app::EraseBaseSource::raster(Arc::clone(&app.conceal_base_cache[&0])),
    );
    app.ai_runtime_init =
        crate::ai::runtime::AiRuntimeInitOwner::failed_for_test("preview routing");
}

#[test]
fn edit_preview_hold_rejects_polygon_confirm_cancel_and_tool_switch_before_canvas() {
    let _input = crate::key_input::lock_test_input();
    for erase in [false, true] {
        for finish_key in [egui::Key::Enter, egui::Key::Escape, egui::Key::B] {
            let (mut app, ctx) = fixture();
            let points = vec![(10.0, 10.0), (30.0, 10.0), (30.0, 30.0)];
            if erase {
                set_erase_mode(&mut app);
                app.erase_tool = crate::app::EraseTool::Polygon;
                app.erase_lasso_points = points;
            } else {
                app.conceal_tool = ConcealTool::Polygon;
                app.conceal_lasso_points = points;
            }
            key_level(SyntheticNavigationKey::Tab, true);
            frame_with_extra_events(
                &mut app,
                &ctx,
                egui::Key::Tab,
                None,
                None,
                &[egui::Event::Key {
                    key: finish_key,
                    physical_key: Some(finish_key),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            let (hold, active, points) = if erase {
                (
                    app.erase_preview_hold,
                    app.erase_preview_active,
                    &app.erase_lasso_points,
                )
            } else {
                (
                    app.conceal_preview_hold,
                    app.conceal_preview_active,
                    &app.conceal_lasso_points,
                )
            };
            assert!(
                points.is_empty(),
                "key handler finished the drawing: {finish_key:?}"
            );
            assert_eq!(
                hold,
                EditPreviewHoldState::BlockedUntilRelease,
                "erase={erase} {finish_key:?}"
            );
            assert!(!active);
            frame(&mut app, &ctx, egui::Key::Tab, None, None);
            assert!(!app.erase_preview_active && !app.conceal_preview_active);
        }
    }
}

#[test]
fn edit_preview_hold_press_on_panel_cannot_become_a_brush_drag_after_key_release() {
    let _input = crate::key_input::lock_test_input();
    for erase in [false, true] {
        let (mut app, ctx) = fixture();
        if erase {
            set_erase_mode(&mut app);
        }
        key_level(SyntheticNavigationKey::Tab, true);
        frame(&mut app, &ctx, egui::Key::Tab, None, None);
        // The erase preview needs a nonempty mask and a retained result. Use the actual
        // preview state here: this test targets pointer ownership, not MI-GAN submission.
        if erase {
            app.erase_preview_active = true;
        }
        let panel = egui::pos2(20.0, 700.0);
        frame(
            &mut app,
            &ctx,
            egui::Key::Tab,
            Some((panel, Some(true))),
            None,
        );
        assert!(app.fs_primary_suppression.is_active(), "erase={erase}");
        key_level(SyntheticNavigationKey::Tab, false);
        frame(&mut app, &ctx, egui::Key::Tab, Some((panel, None)), None);
        let canvas = egui::pos2(700.0, 200.0);
        frame(&mut app, &ctx, egui::Key::Tab, Some((canvas, None)), None);
        frame(&mut app, &ctx, egui::Key::Tab, Some((canvas, None)), None);
        let mask = if erase {
            &app.erase_mask
        } else {
            &app.conceal_mask
        };
        assert!(!mask.as_ref().unwrap().iter().any(|p| *p), "erase={erase}");
        frame(
            &mut app,
            &ctx,
            egui::Key::Tab,
            Some((canvas, Some(false))),
            None,
        );
        frame(
            &mut app,
            &ctx,
            egui::Key::Tab,
            Some((canvas, Some(true))),
            None,
        );
        let mask = if erase {
            &app.erase_mask
        } else {
            &app.conceal_mask
        };
        assert!(mask.as_ref().unwrap().iter().any(|p| *p), "erase={erase}");
    }
}
