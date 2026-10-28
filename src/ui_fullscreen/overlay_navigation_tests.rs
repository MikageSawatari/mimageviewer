//! Edit navigation through the production input and overlay paths, including tool panels.
use super::*;
use crate::key_input::{SyntheticKeyCommand, SyntheticModifiers, SyntheticNavigationKey};

const SIZE: [usize; 2] = [400, 300];

#[derive(Clone, Copy, Debug)]
enum Mode {
    Erase,
    Conceal,
    Crop,
    Text,
    LocalAdjust,
}
const MODES: [Mode; 5] = [
    Mode::Erase,
    Mode::Conceal,
    Mode::Crop,
    Mode::Text,
    Mode::LocalAdjust,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Navigation {
    Pan,
    Scrub,
}

impl Mode {
    fn actions(self) -> (KeyAction, KeyAction) {
        match self {
            Self::Erase => (KeyAction::EraseSpacePan, KeyAction::EraseScrubZoomHold),
            Self::Conceal => (KeyAction::ConcealSpacePan, KeyAction::ConcealScrubZoomHold),
            Self::Crop => (KeyAction::CropSpacePan, KeyAction::CropScrubZoomHold),
            Self::Text => (KeyAction::TextSpacePan, KeyAction::TextScrubZoomHold),
            Self::LocalAdjust => (KeyAction::LaSpacePan, KeyAction::LaScrubZoomHold),
        }
    }
    fn panel(self, app: &App) -> egui::Rect {
        match self {
            Self::Erase => app.erase_panel_rect(viewport()),
            Self::Conceal => app.conceal_panel_rect(viewport()),
            Self::Crop => app.export_crop_panel_rect(viewport()),
            Self::Text => app.text_panel_rect(viewport()),
            Self::LocalAdjust => app.local_adjust_panel_rect(viewport()),
        }
    }
    fn ordinary_drag_started(self, app: &App) -> bool {
        match self {
            Self::Erase => app.erase_last_paint_pos.is_some(),
            Self::Conceal => app.conceal_last_paint_pos.is_some(),
            Self::Crop => app.export_crop_drag.is_some() || app.export_crop_create_drag.is_some(),
            Self::Text => app.text_marquee.is_some() || app.text_drag.is_some(),
            Self::LocalAdjust => app.local_adjust_mask_brush_stroke.is_some(),
        }
    }
}

fn viewport() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 800.0))
}

fn transform(app: &App) -> DisplayedImageTransform {
    DisplayedImageTransform::resolve(DisplayedImageTransformInput {
        pixel_fit: RectPixelFit::Texels,
        page_idx: 0,
        viewport_rect: viewport(),
        source_size: egui::vec2(SIZE[0] as f32, SIZE[1] as f32),
        texture_size: egui::vec2(SIZE[0] as f32, SIZE[1] as f32),
        rotation: crate::rotation_db::Rotation::None,
        free_rotation_rad: 0.0,
        content_bbox: None,
        fit_mode: FullscreenFitInput::legacy(FullscreenFitMode::Page),
        fit_scale_limits: FullscreenFitScaleLimits::default(),
        pixels_per_point: 1.0,
        placement: ResolvedDisplayPlacement::Normal {
            zoom_pan: app.fs_zoom_pan(),
        },
    })
    .unwrap()
}

fn fixture(ctx: &egui::Context, mode: Mode) -> crate::app::AppTestEnvForTest {
    let mut app = crate::app::setup_app_for_test();
    app.items.push(GridItem::Image(std::path::PathBuf::from(
        "C:/test/overlay-navigation.png",
    )));
    app.thumbnails.push(ThumbnailState::Pending);
    app.visible_indices = vec![0];
    app.fullscreen_idx = Some(0);
    app.fs_prev_focused = true;
    app.fs_zoom = 8.0;
    app.settings.fullscreen_navigator_visible = false;
    app.settings.touch_still_chrome_learned = true;
    let pixels = Arc::new(egui::ColorImage::filled(SIZE, egui::Color32::WHITE));
    app.fs_cache.insert(
        0,
        FsCacheEntry::Static {
            pdf_rerender_failures: Vec::new(),
            pdf_metadata: None,
            tex: ctx.load_texture(
                "overlay-navigation-source",
                Arc::clone(&pixels),
                egui::TextureOptions::LINEAR,
            ),
            pixels: Arc::clone(&pixels),
            source_dims: Some(SIZE),
            load_seq: 1,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    match mode {
        Mode::Erase => {
            app.erase_mode = true;
            app.erase_tool = crate::app::EraseTool::Brush;
            app.erase_mask_size = SIZE;
            app.erase_mask = Some(vec![false; SIZE[0] * SIZE[1]]);
            app.erase_base_cache
                .insert(0, crate::app::EraseBaseSource::raster(pixels));
        }
        Mode::Conceal => {
            app.conceal_mode = true;
            app.conceal_tool = crate::conceal::ConcealTool::Brush;
            app.conceal_mask_size = SIZE;
            app.conceal_mask = Some(vec![false; SIZE[0] * SIZE[1]]);
            app.conceal_base_cache.insert(0, pixels);
        }
        Mode::Crop => app.export_crop_mode = true,
        Mode::Text => {
            app.text_mode = true;
            let key = app.page_path_key(0).unwrap();
            app.comic_docs.insert(key, Vec::new());
        }
        Mode::LocalAdjust => {
            app.local_adjust_mode = true;
            app.local_adjust_mask_tool = crate::app::LocalAdjustMaskTool::Brush;
            app.local_adjust_page_layers.insert(
                0,
                Arc::new(vec![local_adjust_core::LocalAdjustmentLayer::new(
                    "navigation test",
                    local_adjust_core::LocalMask::RasterVector(
                        local_adjust_core::RasterVectorMask::empty(SIZE[0], SIZE[1]),
                    ),
                    local_adjust_core::LocalEffect::None,
                )]),
            );
        }
    }
    let (pan, scrub) = mode.actions();
    app.keymap = crate::keymap::Keymap::from_ini_str(&format!(
        "[{}]\n{} = Tab\n{} = Ctrl+Tab\n",
        pan.context().ini_name(),
        pan.ini_name(),
        scrub.ini_name()
    ));
    app
}

fn hold(navigation: Navigation, down: bool) {
    let now = std::time::Instant::now();
    crate::key_input::enqueue_test_synthetic_command(if down {
        SyntheticKeyCommand::down(
            now,
            SyntheticNavigationKey::Tab,
            SyntheticModifiers {
                ctrl: navigation == Navigation::Scrub,
                ..Default::default()
            },
        )
    } else {
        SyntheticKeyCommand::up(now, SyntheticNavigationKey::Tab)
    });
    crate::key_input::advance_test_synthetic_input(now);
}

fn frame(
    app: &mut App,
    ctx: &egui::Context,
    mode: Mode,
    pos: egui::Pos2,
    edge: Option<bool>,
    modifiers: egui::Modifiers,
) {
    install_fs_navigator_input_tracking(ctx);
    app.frame_counter += 1;
    let mut events = vec![
        egui::Event::ModifiersChanged(modifiers),
        egui::Event::PointerMoved(pos),
    ];
    if let Some(pressed) = edge {
        events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        });
    }
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(viewport()),
            focused: true,
            events,
            time: Some(app.frame_counter as f64 * 0.1),
            ..Default::default()
        },
        |ui| {
            app.keyboard_owner_for_pass(ctx);
            let transform = transform(app);
            app.fullscreen_page_layout
                .begin(FullscreenPageLayoutKind::Single);
            app.fullscreen_page_layout.push(transform);
            let state = FsFrameState {
                is_video: false,
                original_preview_active: false,
                page_turn_decision: FsPageTurnDecision::normal(),
                tex: None,
                thumb_tex: None,
                location_display: String::new(),
                image_dims: Some((SIZE[0] as u32, SIZE[1] as u32)),
                image_file_size: None,
                image_downscaled: false,
                is_loading: false,
                vst3_waiting_for_video: false,
                fs_load_failed: false,
                pdf_content_type: None,
            };
            let (page_nav, close) =
                app.handle_fs_wheel_and_click(ui, ctx, viewport(), &state, false, 0, &mut None);
            assert!(matches!(page_nav, FsPageNav::None));
            assert!(!close);
            match mode {
                Mode::Erase => {
                    app.handle_erase_paint(ctx, &transform, Default::default());
                    app.draw_erase_overlay(ui, ctx, &transform);
                }
                Mode::Conceal => {
                    app.handle_conceal_paint(ctx, &transform, Default::default());
                    app.draw_conceal_overlay(ui, ctx, &transform);
                }
                Mode::Crop => {
                    let allowed = !app.any_dialog_open() && !mode.panel(app).contains(pos);
                    app.draw_export_crop_overlay(
                        ui,
                        &transform,
                        0,
                        SIZE,
                        allowed,
                        Default::default(),
                    );
                    app.draw_export_crop_panel(ctx, viewport(), 0, Some(SIZE));
                }
                Mode::Text => {
                    app.handle_text_canvas_input(ctx, &transform, Default::default());
                    app.draw_text_overlay(ui, ctx, &transform);
                }
                Mode::LocalAdjust => {
                    app.handle_local_adjust_canvas_input(
                        ctx,
                        viewport(),
                        &transform,
                        Default::default(),
                    );
                    app.draw_local_adjust_canvas_overlay(ui, &transform);
                    app.draw_local_adjust_panel(ctx, viewport());
                }
            }
        },
    )
    .drop_without_applying_deltas();
}

fn crossing_panel_case(mode: Mode, navigation: Navigation) {
    crate::key_input::arm_test_synthetic_input(0xE030, egui::ViewportId::ROOT);
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let mut app = fixture(&ctx, mode);
    let canvas = viewport().center();
    // Render the actual panels before testing their input exclusion and floating layers.
    for _ in 0..2 {
        frame(&mut app, &ctx, mode, canvas, None, egui::Modifiers::NONE);
    }
    let panel = mode.panel(&app);
    let on_panel = panel.min + egui::vec2(18.0, 12.0);
    let further_on_panel = on_panel + egui::vec2(60.0, 0.0);
    assert!(panel.contains(on_panel) && panel.contains(further_on_panel));
    assert!(!panel.contains(canvas));
    hold(navigation, true);
    let modifiers = if navigation == Navigation::Scrub {
        egui::Modifiers::CTRL
    } else {
        egui::Modifiers::NONE
    };
    frame(&mut app, &ctx, mode, canvas, Some(true), modifiers);
    assert!(
        !mode.ordinary_drag_started(&app),
        "{mode:?} {navigation:?}: navigation must own the press"
    );
    match navigation {
        Navigation::Scrub => assert!(app.fs_middle_zoom_drag.is_some()),
        Navigation::Pan => assert!(app.fs_pan_drag_start.is_some()),
    }
    let initial = (app.fs_zoom, app.fs_pan);
    frame(&mut app, &ctx, mode, on_panel, None, modifiers);
    let first = (app.fs_zoom, app.fs_pan);
    assert_ne!(
        first, initial,
        "{mode:?} {navigation:?}: must continue on panel"
    );
    frame(&mut app, &ctx, mode, further_on_panel, None, modifiers);
    assert_ne!(
        (app.fs_zoom, app.fs_pan),
        first,
        "{mode:?} {navigation:?}: must keep moving on panel"
    );
    assert!(!mode.ordinary_drag_started(&app));
    // Both the binding and primary are released over the panel.
    hold(navigation, false);
    frame(
        &mut app,
        &ctx,
        mode,
        further_on_panel,
        Some(false),
        egui::Modifiers::NONE,
    );
    assert!(
        app.fs_middle_zoom_drag.is_none(),
        "{mode:?}: scrub release must retire"
    );
    assert!(
        app.fs_pan_drag_start.is_none(),
        "{mode:?}: pan release must retire"
    );
    frame(
        &mut app,
        &ctx,
        mode,
        canvas,
        Some(true),
        egui::Modifiers::NONE,
    );
    assert!(
        mode.ordinary_drag_started(&app),
        "{mode:?} {navigation:?}: next press must reach the tool"
    );
    assert!(app.fs_middle_zoom_drag.is_none() && app.fs_pan_drag_start.is_none());
    frame(
        &mut app,
        &ctx,
        mode,
        canvas,
        Some(false),
        egui::Modifiers::NONE,
    );
    // A new panel press still cannot start navigation.
    hold(navigation, true);
    frame(&mut app, &ctx, mode, on_panel, Some(true), modifiers);
    assert!(
        app.fs_middle_zoom_drag.is_none() && app.fs_pan_drag_start.is_none(),
        "{mode:?}: panel start must be rejected"
    );
    hold(navigation, false);
    frame(
        &mut app,
        &ctx,
        mode,
        on_panel,
        Some(false),
        egui::Modifiers::NONE,
    );
    crate::key_input::clear_test_synthetic_input();
}

#[test]
fn scrub_zoom_actual_edit_overlays_continue_and_release_over_panels() {
    let _input = crate::key_input::lock_test_input();
    for mode in MODES {
        crossing_panel_case(mode, Navigation::Scrub);
    }
}

#[test]
fn space_pan_actual_edit_overlays_continue_and_release_over_panels() {
    let _input = crate::key_input::lock_test_input();
    for mode in MODES {
        crossing_panel_case(mode, Navigation::Pan);
    }
}
