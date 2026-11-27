use super::tests::{
    mask_detect_fixture, mask_detect_install_source, mask_detect_output, mask_detect_queue,
};
use super::*;
use crate::displayed_image_transform::{
    DisplayedImageTransform, DisplayedImageTransformInput, FullscreenFitScaleLimits, RectPixelFit,
    ResolvedDisplayPlacement,
};

fn transform() -> DisplayedImageTransform {
    DisplayedImageTransform::from_resolved_rect(
        DisplayedImageTransformInput {
            page_idx: 0,
            viewport_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0)),
            source_size: egui::vec2(8.0, 8.0),
            texture_size: egui::vec2(8.0, 8.0),
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
        egui::Rect::from_min_size(egui::pos2(600.0, 200.0), egui::vec2(200.0, 200.0)),
    )
    .unwrap()
}

fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    }
}

fn draw_mask_detect_real_panel(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    tool: MaskDetectTool,
    transform: &DisplayedImageTransform,
) {
    match tool {
        MaskDetectTool::Erase => app.draw_erase_overlay(ui, ctx, transform),
        MaskDetectTool::Conceal => app.draw_conceal_panel(ctx, transform.viewport_rect),
        MaskDetectTool::LocalAdjust => app.draw_local_adjust_panel(ctx, transform.viewport_rect),
    }
}

fn draw_mask_detect_layer_frame(
    app: &mut App,
    ctx: &egui::Context,
    tool: MaskDetectTool,
    transform: &DisplayedImageTransform,
    events: Vec<egui::Event>,
) {
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(transform.viewport_rect),
            events,
            ..Default::default()
        },
        |ui| {
            // Draw the real panels after the dialog too: Order, not call order or
            // the most recently clicked Area, must keep the dialog above them.
            app.show_mask_detect_dialog(ctx);
            draw_mask_detect_real_panel(app, ui, ctx, tool, transform);
        },
    )
    .drop_without_applying_deltas();
}

fn assert_mask_detect_layer_order_above(
    ctx: &egui::Context,
    dialog_id: &str,
    lower_ids: &[&str],
) -> egui::LayerId {
    let layers: Vec<_> = ctx.memory(|memory| memory.layer_ids().collect());
    let dialog_index = layers
        .iter()
        .position(|layer| layer.id == egui::Id::unique(dialog_id))
        .expect("dialog layer exists");
    let dialog = layers[dialog_index];
    assert_eq!(dialog.order, egui::Order::Foreground);
    for id in lower_ids {
        let index = layers
            .iter()
            .position(|layer| layer.id == egui::Id::unique(id))
            .unwrap_or_else(|| panic!("missing lower layer {id}"));
        assert!(
            index < dialog_index,
            "{id} must be below {dialog_id}: {layers:?}"
        );
    }
    dialog
}

fn assert_mask_detect_layer_above(ctx: &egui::Context, dialog_id: &str, lower_ids: &[&str]) {
    let dialog = assert_mask_detect_layer_order_above(ctx, dialog_id, lower_ids);
    assert_eq!(ctx.memory(|memory| memory.top_modal_layer()), Some(dialog));
    assert!(ctx.memory(|memory| memory.allows_interaction(dialog)));
    let rect = ctx.memory(|memory| memory.area_rect(dialog.id)).unwrap();
    let mut checked_overlap = false;
    for id in lower_ids {
        let lower = egui::LayerId::new(egui::Order::Foreground, egui::Id::unique(id));
        assert!(
            !ctx.memory(|memory| memory.allows_interaction(lower)),
            "{id} must be blocked by the modal"
        );
        let lower_rect = ctx.memory(|memory| memory.area_rect(lower.id)).unwrap();
        let overlap = rect.intersect(lower_rect);
        if overlap.is_positive() {
            assert_eq!(
                ctx.layer_id_at(overlap.center()),
                Some(dialog),
                "{dialog_id} must own the overlap with {id}"
            );
            checked_overlap = true;
        }
    }
    assert!(
        checked_overlap,
        "fixture must actually overlap the dialog and a lower surface"
    );
}

fn mask_detect_raise_count(ctx: &egui::Context, name: &str) -> usize {
    let key = egui::Id::unique((
        "mask_detect_window_raises",
        ctx.viewport_id(),
        egui::Id::unique(name),
    ));
    ctx.data(|data| data.get_temp::<Vec<u64>>(key).unwrap_or_default().len())
}

#[test]
fn mask_detect_reopened_from_real_panel_click_recovers_same_pass_raise_tie_for_three_tools() {
    let _input = crate::key_input::lock_test_input();
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        for together in [true, false] {
            let mut app = mask_detect_fixture(tool);
            app.erase_tool = EraseTool::Brush;
            app.conceal_tool = crate::conceal::ConcealTool::Brush;
            app.local_adjust_mask_tool = LocalAdjustMaskTool::Brush;
            let (_catalog_tx, rx) = mpsc::channel();
            *app.mask_detect_catalog = DetectorCatalogState::Loading {
                cancel: Arc::new(AtomicBool::new(false)),
                rx,
            };
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            let transform = transform();
            let (start_id, brush_id, start_panel_id, brush_panel_id, panels): (
                _,
                _,
                _,
                _,
                &[&str],
            ) = match tool {
                MaskDetectTool::Erase => (
                    "mask_detect_erase_start_rect",
                    "mask_detect_erase_brush_rect",
                    "erase_tool_panel",
                    "erase_tool_panel",
                    &["erase_tool_panel"],
                ),
                MaskDetectTool::Conceal => (
                    "mask_detect_conceal_start_rect",
                    "mask_detect_conceal_brush_rect",
                    "conceal_panel",
                    "conceal_panel",
                    &["conceal_panel"],
                ),
                MaskDetectTool::LocalAdjust => (
                    "mask_detect_local_start_rect",
                    "mask_detect_local_brush_rect",
                    "local_adjust_tool_panel",
                    "local_adjust_panel",
                    &["local_adjust_panel", "local_adjust_tool_panel"],
                ),
            };
            let start_panel =
                egui::LayerId::new(egui::Order::Foreground, egui::Id::unique(start_panel_id));
            let brush_panel =
                egui::LayerId::new(egui::Order::Foreground, egui::Id::unique(brush_panel_id));
            let button_snapshot = |name| {
                ctx.data(|data| {
                    data.get_temp::<(egui::Id, egui::Rect, bool)>(egui::Id::unique(name))
                })
                .unwrap()
            };
            let frame = |app: &mut App, events| {
                let mut start_clicked = false;
                let mut targets = Vec::new();
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(transform.viewport_rect),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        // Production order: the real panel button opens the Window
                        // before the Window is drawn later in this SAME pass.
                        draw_mask_detect_real_panel(app, ui, &ctx, tool, &transform);
                        // Keep witnesses across any sizing/discard pass. Inspect
                        // clicked before registering the newly opened modal.
                        start_clicked |= button_snapshot(start_id).2;
                        targets.extend(ctx.pointer_button_targets());
                        app.show_mask_detect_dialog(&ctx);
                    },
                )
                .drop_without_applying_deltas();
                (start_clicked, targets)
            };
            let button = |name, panel| {
                let (id, rect, _) = button_snapshot(name);
                // A layout rect can lie outside a ScrollArea's clip. Require
                // the actual enabled widget to cover the point we will click.
                let response = ctx.read_response(id).unwrap();
                let point = rect.center();
                assert_eq!(response.rect, rect);
                assert_eq!(response.layer_id, panel, "{tool:?}: {name} widget layer");
                assert!(
                    response.enabled() && response.interact_rect.contains(point),
                    "{tool:?}, together={together}: {name} must be clickable: {response:?}"
                );
                assert_eq!(
                    ctx.layer_id_at(point),
                    Some(panel),
                    "{tool:?}, together={together}: {name} must be exposed"
                );
                point
            };
            for _ in 0..4 {
                frame(&mut app, Vec::new());
            }
            let start = button(start_id, start_panel);
            frame(&mut app, vec![egui::Event::PointerMoved(start)]);
            for pressed in [true, false] {
                frame(&mut app, mask_detect_pointer(start, pressed));
            }
            assert_eq!(app.mask_detect_dialog, Some(tool));
            for _ in 0..4 {
                frame(&mut app, Vec::new());
            }
            assert_mask_detect_layer_above(&ctx, "mask_detect_settings", panels);
            let close = ctx
                .data(|data| {
                    data.get_temp::<Vec<egui::Rect>>(egui::Id::unique("mask_detect_actions"))
                })
                .unwrap()[1]
                .center();
            frame(&mut app, vec![egui::Event::PointerMoved(close)]);
            for pressed in [true, false] {
                frame(&mut app, mask_detect_pointer(close, pressed));
            }
            assert!(app.mask_detect_dialog.is_none());
            for _ in 0..3 {
                frame(&mut app, Vec::new());
            }
            let start_before_brush = button_snapshot(start_id).1;
            let brush = button(brush_id, brush_panel);
            // Moving from Close to Brush in the press frame gives the native
            // drag-scroll background that entire pointer delta. Move with the
            // button up first, so this fixture clicks rather than scrolls.
            frame(&mut app, vec![egui::Event::PointerMoved(brush)]);
            assert_eq!(button(brush_id, brush_panel), brush);
            for pressed in [true, false] {
                frame(&mut app, mask_detect_pointer(brush, pressed));
            }
            assert!(
                button_snapshot(brush_id).2,
                "the selected brush button must receive the click"
            );
            assert_eq!(
                button_snapshot(start_id).1,
                start_before_brush,
                "{tool:?}: a stationary brush click must not move the detection button"
            );
            // The brush raises its own panel above the retained Window. Local
            // adjustment puts detection in a DIFFERENT panel, still below the
            // Window; raising that panel and the Window together preserves the
            // correct order instead of requiring a tie repair.
            let layers: Vec<_> = ctx.memory(|memory| memory.layer_ids().collect());
            let window_index = layers
                .iter()
                .position(|layer| layer.id == egui::Id::unique("mask_detect_settings"))
                .unwrap();
            let brush_panel_index = layers
                .iter()
                .position(|layer| *layer == brush_panel)
                .unwrap();
            assert!(
                window_index < brush_panel_index,
                "{tool:?}: the brush must raise its panel above the retained Window"
            );
            let start_panel_index = layers
                .iter()
                .position(|layer| *layer == start_panel)
                .unwrap();
            let needs_tie_repair = window_index < start_panel_index;
            assert_eq!(
                needs_tie_repair,
                tool != MaskDetectTool::LocalAdjust,
                "{tool:?}: only same-panel Brush and Detect recreate the stale opening order"
            );
            let before = mask_detect_raise_count(&ctx, "mask_detect_settings");
            let start = button(start_id, start_panel);
            frame(&mut app, vec![egui::Event::PointerMoved(start)]);
            assert_eq!(button(start_id, start_panel), start);
            let start_widget = button_snapshot(start_id).0;
            let mut received_click = false;
            let mut click_targets = Vec::new();
            if together {
                let mut events = mask_detect_pointer(start, true);
                events.extend(mask_detect_pointer(start, false));
                let (clicked, targets) = frame(&mut app, events);
                received_click |= clicked;
                click_targets.extend(targets);
            } else {
                for pressed in [true, false] {
                    let (clicked, targets) = frame(&mut app, mask_detect_pointer(start, pressed));
                    received_click |= clicked;
                    click_targets.extend(targets);
                }
            }
            assert_eq!(
                click_targets,
                vec![
                    (
                        start,
                        egui::PointerButton::Primary,
                        true,
                        Some(start_widget),
                        true
                    ),
                    (
                        start,
                        egui::PointerButton::Primary,
                        false,
                        Some(start_widget),
                        true
                    ),
                ],
                "{tool:?}, together={together}: both edges must target the real detection button"
            );
            assert!(
                received_click,
                "{tool:?}, together={together}: the real detection Response must be clicked"
            );
            assert_eq!(
                app.mask_detect_dialog,
                Some(tool),
                "{tool:?}, together={together}: real button must open the Window"
            );
            if tool == MaskDetectTool::LocalAdjust && together {
                // Detect's right panel was below the Window before reopening.
                // The stable tie already puts the Window above BOTH panels in
                // the opening frame, without a corrective move_to_top.
                assert_mask_detect_layer_above(&ctx, "mask_detect_settings", panels);
            }
            // Area may tie with the panel in the opening pass. Reconciliation
            // repairs the wrong order and stops once the invariant holds.
            for _ in 0..6 {
                frame(&mut app, Vec::new());
                assert_mask_detect_layer_above(&ctx, "mask_detect_settings", panels);
            }
            let repaired = mask_detect_raise_count(&ctx, "mask_detect_settings");
            if together || !needs_tie_repair {
                assert_eq!(repaired, before + usize::from(needs_tie_repair));
            } else {
                assert!(repaired <= before + 1);
            }
            for _ in 0..4 {
                frame(&mut app, Vec::new());
            }
            assert_eq!(
                mask_detect_raise_count(&ctx, "mask_detect_settings"),
                repaired,
                "do not raise an already-correct Window"
            );
            eprintln!("{tool:?}, together={together}: real panel click and layer checks completed");
        }
    }
}

#[test]
fn mask_detect_front_layer_invariant_repairs_after_upper_modal_closes_for_settings_and_progress() {
    let _input = crate::key_input::lock_test_input();
    for progress in [false, true] {
        let mut app = mask_detect_fixture(MaskDetectTool::Erase);
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let name = if progress {
            "mask_detect_progress"
        } else {
            "mask_detect_settings"
        };
        let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::unique(name));
        let upper = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("external_tool_preparing"),
        );
        let panel = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("erase_tool_panel"),
        );
        let transform = transform();
        let open = |app: &mut App| {
            if progress {
                let request = app
                    .capture_mask_detect_request(MaskDetectTool::Erase)
                    .unwrap();
                mask_detect_queue(app, request, mask_detect_output());
            } else {
                app.mask_detect_dialog = Some(MaskDetectTool::Erase);
            }
        };
        let frame = |app: &mut App, upper_visible, disrupt| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(transform.viewport_rect),
                    ..Default::default()
                },
                |ui| {
                    draw_mask_detect_real_panel(app, ui, &ctx, MaskDetectTool::Erase, &transform);
                    app.show_mask_detect_dialog(&ctx);
                    if upper_visible {
                        egui::Modal::new(upper.id).show(&ctx, |ui| {
                            crate::external_tool::draw_external_tool_preparing_body(ui, 1);
                        });
                    }
                    if disrupt {
                        // Inject a wrong order independently of display start.
                        // Keep the legitimate upper Modal above both layers.
                        ctx.move_to_top(panel);
                        if upper_visible {
                            ctx.move_to_top(upper);
                        }
                    }
                },
            )
            .drop_without_applying_deltas();
        };
        open(&mut app);
        for _ in 0..6 {
            frame(&mut app, false, false);
        }
        assert_mask_detect_layer_above(&ctx, name, &["erase_tool_panel"]);
        let before = mask_detect_raise_count(&ctx, name);
        app.cancel_mask_detect();
        for _ in 0..3 {
            frame(&mut app, false, false);
        }
        open(&mut app);
        // Keep the earlier regression too: another Modal becomes visible on
        // the same pass that this retained Window id reappears.
        for pass in 0..6 {
            frame(&mut app, true, false);
            let layers: Vec<_> = ctx.memory(|memory| memory.layer_ids().collect());
            assert!(
                layers.iter().position(|l| *l == layer).unwrap()
                    < layers.iter().position(|l| *l == upper).unwrap()
            );
            if pass >= 1 {
                assert_eq!(ctx.memory(|memory| memory.top_modal_layer()), Some(upper));
            }
        }
        frame(&mut app, true, true);
        let layers: Vec<_> = ctx.memory(|memory| memory.layer_ids().collect());
        assert!(
            layers.iter().position(|l| *l == layer).unwrap()
                < layers.iter().position(|l| *l == panel).unwrap()
        );
        assert!(
            layers.iter().position(|l| *l == panel).unwrap()
                < layers.iter().position(|l| *l == upper).unwrap()
        );
        for _ in 0..6 {
            frame(&mut app, true, false);
        }
        assert_eq!(
            mask_detect_raise_count(&ctx, name),
            before,
            "never overtake the upper Modal to repair panel order"
        );
        for _ in 0..6 {
            frame(&mut app, false, false);
        }
        assert_mask_detect_layer_above(&ctx, name, &["erase_tool_panel"]);
        assert_eq!(
            mask_detect_raise_count(&ctx, name),
            before + 1,
            "repair when the upper Modal is gone, without reopening"
        );
        // A later violation is repaired too; no First/Visible phase is needed.
        frame(&mut app, false, true);
        for _ in 0..6 {
            frame(&mut app, false, false);
        }
        assert_mask_detect_layer_above(&ctx, name, &["erase_tool_panel"]);
        assert_eq!(
            mask_detect_raise_count(&ctx, name),
            before + 2,
            "repair a later violation, then stop raising once correct"
        );
    }
}

#[test]
fn mask_detect_settings_and_progress_layers_stay_above_real_panels_after_reopen_and_tool_switch() {
    let _input = crate::key_input::lock_test_input();
    // One Context retains both Window IDs and Area stacking history across all
    // fixtures, including the reported conceal -> erase transition.
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let transform = transform();
    for tool in [
        MaskDetectTool::Conceal,
        MaskDetectTool::Erase,
        MaskDetectTool::LocalAdjust,
        MaskDetectTool::Conceal,
        MaskDetectTool::Erase,
        MaskDetectTool::LocalAdjust,
    ] {
        let mut app = mask_detect_fixture(tool);
        let (_catalog_tx, rx) = mpsc::channel();
        *app.mask_detect_catalog = DetectorCatalogState::Loading {
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
        };
        let panels: &[&str] = match tool {
            MaskDetectTool::Erase => &["erase_tool_panel"],
            MaskDetectTool::Conceal => &["conceal_panel"],
            MaskDetectTool::LocalAdjust => &["local_adjust_panel", "local_adjust_tool_panel"],
        };
        for _ in 0..2 {
            app.cancel_mask_detect();
            for _ in 0..3 {
                draw_mask_detect_layer_frame(&mut app, &ctx, tool, &transform, Vec::new());
            }
            let panel = egui::LayerId::new(egui::Order::Foreground, egui::Id::unique(panels[0]));
            let point =
                ctx.memory(|memory| memory.area_rect(panel.id)).unwrap().min + egui::vec2(2.0, 2.0);
            assert_eq!(ctx.layer_id_at(point), Some(panel));
            // A real headless press on the panel background exercises egui's
            // bring-to-front behavior without activating a tool button.
            for pressed in [true, false] {
                draw_mask_detect_layer_frame(
                    &mut app,
                    &ctx,
                    tool,
                    &transform,
                    vec![
                        egui::Event::PointerMoved(point),
                        egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
            app.open_mask_detect_dialog(tool);
            assert_eq!(app.mask_detect_dialog, Some(tool));
            for _ in 0..4 {
                draw_mask_detect_layer_frame(&mut app, &ctx, tool, &transform, Vec::new());
                assert_mask_detect_layer_order_above(&ctx, "mask_detect_settings", panels);
            }
            assert_mask_detect_layer_above(&ctx, "mask_detect_settings", panels);
            let request = app.capture_mask_detect_request(tool).unwrap();
            mask_detect_queue(&mut app, request, mask_detect_output());
            app.mask_detect_dialog = None;
            for _ in 0..4 {
                draw_mask_detect_layer_frame(&mut app, &ctx, tool, &transform, Vec::new());
                assert_mask_detect_layer_order_above(&ctx, "mask_detect_progress", panels);
            }
            assert_mask_detect_layer_above(&ctx, "mask_detect_progress", panels);
        }
    }
}

#[test]
fn mask_detect_windows_stay_above_conceal_panel_on_immediate_open_and_clicks_during_progress() {
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let transform = transform();
    // Retain Window/Area history for both the first open and subsequent opens.
    for click_button in [false, true, true] {
        let tool = MaskDetectTool::Conceal;
        let mut app = mask_detect_fixture(tool);
        let (_catalog_tx, rx) = mpsc::channel();
        *app.mask_detect_catalog = DetectorCatalogState::Loading {
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
        };
        // Complete egui's initial measurement passes, with no intervening
        // operation between showing/clicking the panel and opening detection.
        for _ in 0..3 {
            draw_mask_detect_layer_frame(&mut app, &ctx, tool, &transform, Vec::new());
        }
        let select = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::unique("conceal_select_button_test_rect"))
            })
            .unwrap();
        let point = select.center();
        let panel = egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("conceal_panel"));
        assert_eq!(ctx.layer_id_at(point), Some(panel));
        let pointer = |pressed| {
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ]
        };
        if click_button {
            app.conceal_tool = crate::conceal::ConcealTool::Brush;
            for pressed in [true, false] {
                draw_mask_detect_layer_frame(&mut app, &ctx, tool, &transform, pointer(pressed));
            }
            assert_eq!(
                app.conceal_tool,
                crate::conceal::ConcealTool::Select,
                "the real panel button must receive the click"
            );
        }
        // The very next frame opens the dialog via the normal entry boundary.
        app.open_mask_detect_dialog(tool);
        assert_eq!(app.mask_detect_dialog, Some(tool));
        for _ in 0..4 {
            draw_mask_detect_layer_frame(&mut app, &ctx, tool, &transform, Vec::new());
            assert_mask_detect_layer_order_above(&ctx, "mask_detect_settings", &["conceal_panel"]);
        }
        assert_mask_detect_layer_above(&ctx, "mask_detect_settings", &["conceal_panel"]);
        let request = app.capture_mask_detect_request(tool).unwrap();
        mask_detect_queue(&mut app, request, mask_detect_output());
        app.mask_detect_dialog = None;
        app.conceal_tool = crate::conceal::ConcealTool::Brush;
        for _ in 0..4 {
            draw_mask_detect_layer_frame(&mut app, &ctx, tool, &transform, Vec::new());
            assert_mask_detect_layer_order_above(&ctx, "mask_detect_progress", &["conceal_panel"]);
        }
        // This exposed panel button lies outside the smaller progress window.
        // Area-level interactions must not raise it above the modal window,
        // and the modal gate must keep the underlying tool button disabled.
        let progress = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("mask_detect_progress"),
        );
        assert!(
            !ctx.memory(|memory| memory.area_rect(progress.id))
                .unwrap()
                .contains(point)
        );
        // egui reroutes hits below the modal, even outside the window's rect.
        assert_eq!(ctx.layer_id_at(point), Some(progress));
        for pressed in [true, false] {
            draw_mask_detect_layer_frame(&mut app, &ctx, tool, &transform, pointer(pressed));
            assert_mask_detect_layer_above(&ctx, "mask_detect_progress", &["conceal_panel"]);
            assert_eq!(app.conceal_tool, crate::conceal::ConcealTool::Brush);
            assert!(app.mask_detect_pending.is_some());
        }
        app.cancel_mask_detect();
        // Close before the next fixture reuses the retained layer history.
        draw_mask_detect_layer_frame(&mut app, &ctx, tool, &transform, Vec::new());
    }
}

fn mask_detect_text_paint(output: &egui::FullOutput, text: &str) -> (usize, egui::Rect) {
    output
        .shapes
        .iter()
        .enumerate()
        .find_map(|(index, clipped)| {
            if let egui::Shape::Text(shape) = &clipped.shape {
                if shape.galley.text() == text {
                    return Some((index, shape.galley.rect.translate(shape.pos.to_vec2())));
                }
            }
            None
        })
        .unwrap_or_else(|| panic!("missing painted text: {text}"))
}

fn mask_detect_pointer(point: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(point),
        egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        },
    ]
}

#[test]
fn mask_detect_foreground_windows_keep_real_viewer_toast_above_and_action_clickable() {
    let _input = crate::key_input::lock_test_input();
    for progress in [false, true] {
        let mut app = mask_detect_fixture(MaskDetectTool::Erase);
        let (id, title) = if progress {
            let request = app
                .capture_mask_detect_request(MaskDetectTool::Erase)
                .unwrap();
            mask_detect_queue(&mut app, request, mask_detect_output());
            ("mask_detect_progress", "検出中")
        } else {
            app.mask_detect_dialog = Some(MaskDetectTool::Erase);
            ("mask_detect_settings", "自動検出")
        };
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let rect = transform().viewport_rect;
        for _ in 0..4 {
            draw_mask_detect_layer_frame(
                &mut app,
                &ctx,
                MaskDetectTool::Erase,
                &transform(),
                Vec::new(),
            );
        }
        let dialog = egui::LayerId::new(egui::Order::Foreground, egui::Id::unique(id));
        // Move the real Window underneath the real toast (which is at top-right).
        let mut state = egui::AreaState::load(&ctx, dialog.id).unwrap();
        state.set_left_top_pos(egui::pos2(rect.right() - state.rect().width() - 20.0, 40.0));
        ctx.memory_mut(|memory| memory.areas_mut().set_state(dialog, state));
        let message = "検出器フォルダを開けませんでした";
        app.show_mask_detect_error(message.into());
        let mut toast_text_rect = egui::Rect::NOTHING;
        for _ in 0..4 {
            app.fs_feedback_toast.as_mut().unwrap().1 = std::time::Instant::now();
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| {
                    // Calling toast first must still paint it on top of the Window.
                    app.draw_feedback_toast(ui, rect, &ctx, ActionSurface::Viewer);
                    app.show_mask_detect_dialog(&ctx);
                },
            );
            let (toast_index, text_rect) = mask_detect_text_paint(&output, message);
            let (window_index, _) = mask_detect_text_paint(&output, title);
            assert!(
                toast_index > window_index,
                "toast must paint after the overlapping Window"
            );
            toast_text_rect = text_rect;
            output.drop_without_applying_deltas();
        }
        let window = ctx.memory(|memory| memory.area_rect(dialog.id)).unwrap();
        assert!(window.intersect(toast_text_rect).is_positive());
        let toast_layer = egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("feedback_toast_viewer"),
        );
        assert!(ctx.memory(|memory| memory.allows_interaction(toast_layer)));
        // Use the production action Response on the real toast's text rectangle.
        // Do not activate the action: that would start Explorer in a unit test.
        let action = crate::feedback::FeedbackAction::RevealPath(app.tmp.path().join("result.png"));
        let mut clicked = false;
        for pressed in [None, Some(true), Some(false)] {
            app.fs_feedback_toast.as_mut().unwrap().1 = std::time::Instant::now();
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    events: pressed
                        .map(|p| mask_detect_pointer(toast_text_rect.center(), p))
                        .unwrap_or_default(),
                    ..Default::default()
                },
                |ui| {
                    app.show_mask_detect_dialog(&ctx);
                    app.draw_feedback_toast(ui, rect, &ctx, ActionSurface::Viewer);
                    let toast_ui =
                        ui.new_child(egui::UiBuilder::new().layer_id(toast_layer).max_rect(rect));
                    clicked |= action.response(&toast_ui, toast_text_rect).clicked();
                },
            )
            .drop_without_applying_deltas();
        }
        assert!(
            clicked,
            "a Tooltip-layer toast action must remain clickable above the modal"
        );
    }
}

#[test]
fn mask_detect_foreground_modal_keeps_real_margin_hover_tooltip_above_window() {
    let _input = crate::key_input::lock_test_input();
    let mut app = mask_detect_fixture(MaskDetectTool::Erase);
    app.mask_detect_dialog = Some(MaskDetectTool::Erase);
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    ctx.all_styles_mut(|style| {
        style.interaction.tooltip_delay = 0.0;
        style.interaction.show_tooltips_only_when_still = false;
    });
    let transform = transform();
    for _ in 0..4 {
        draw_mask_detect_layer_frame(
            &mut app,
            &ctx,
            MaskDetectTool::Erase,
            &transform,
            Vec::new(),
        );
    }
    let margin = ctx
        .data(|d| d.get_temp::<egui::Rect>(egui::Id::unique("mask_detect_box_margin_rect")))
        .unwrap();
    let point = egui::pos2(margin.left() + 8.0, margin.center().y);
    let tooltip = "幅・高さの指定割合を各辺の外側に足します";
    let mut output: Option<egui::FullOutput> = None;
    for _ in 0..6 {
        let next = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(transform.viewport_rect),
                events: vec![egui::Event::PointerMoved(point)],
                ..Default::default()
            },
            |_| app.show_mask_detect_dialog(&ctx),
        );
        if let Some(old) = output.replace(next) {
            old.drop_without_applying_deltas();
        }
    }
    let output = output.unwrap();
    let (tooltip_index, tooltip_rect) = mask_detect_text_paint(&output, tooltip);
    let (window_index, _) = mask_detect_text_paint(&output, "自動検出");
    assert!(tooltip_index > window_index);
    let window = ctx
        .memory(|memory| memory.area_rect(egui::Id::unique("mask_detect_settings")))
        .unwrap();
    assert!(
        window.intersect(tooltip_rect).is_positive(),
        "hover tooltip must actually overlap the Window"
    );
    assert!(ctx.memory(|memory| {
        memory
            .layer_ids()
            .any(|layer| layer.order == egui::Order::Tooltip && memory.allows_interaction(layer))
    }));
    output.drop_without_applying_deltas();
}

#[test]
fn mask_detect_foreground_modal_allows_menu_and_combo_popup_selection() {
    let _input = crate::key_input::lock_test_input();
    for menu in [true, false] {
        let mut app = mask_detect_fixture(MaskDetectTool::Erase);
        app.mask_detect_dialog = Some(MaskDetectTool::Erase);
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let screen = transform().viewport_rect;
        let dialog = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("mask_detect_settings"),
        );
        let mut anchor = egui::Rect::NOTHING;
        let mut item = None;
        let mut selected = false;
        let mut frame = |app: &mut App, events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app.show_mask_detect_dialog(&ctx);
                    // Representative egui controls use the actual dialog's layer.
                    // No new menu or combo is added to the product dialog.
                    let mut child = ui.new_child(egui::UiBuilder::new().layer_id(dialog).max_rect(
                        egui::Rect::from_min_size(
                            egui::pos2(100.0, 100.0),
                            egui::vec2(240.0, 60.0),
                        ),
                    ));
                    if menu {
                        anchor = child
                            .menu_button("Test menu", |ui| {
                                let response = ui.button("Choose");
                                item = Some((response.rect, ui.layer_id()));
                                selected |= response.clicked();
                            })
                            .response
                            .rect;
                    } else {
                        anchor = egui::ComboBox::from_id_salt("mask_detect_test_combo")
                            .selected_text("Choose")
                            .show_ui(&mut child, |ui| {
                                let response = ui.selectable_label(selected, "Item");
                                item = Some((response.rect, ui.layer_id()));
                                selected |= response.clicked();
                            })
                            .response
                            .rect;
                    }
                },
            )
            .drop_without_applying_deltas();
            (anchor, item, selected)
        };
        let mut button = egui::Rect::NOTHING;
        for _ in 0..4 {
            button = frame(&mut app, Vec::new()).0;
        }
        let raises_before_popup = mask_detect_raise_count(&ctx, "mask_detect_settings");
        for pressed in [true, false] {
            frame(&mut app, mask_detect_pointer(button.center(), pressed));
        }
        let mut popup = None;
        for _ in 0..4 {
            popup = frame(&mut app, Vec::new()).1;
        }
        let (item_rect, layer) = popup.expect("real egui popup must open");
        assert_ne!(layer, dialog);
        assert_eq!(layer.order, egui::Order::Foreground);
        let layers: Vec<_> = ctx.memory(|memory| memory.layer_ids().collect());
        assert!(
            layers.iter().position(|l| *l == dialog).unwrap()
                < layers.iter().position(|l| *l == layer).unwrap(),
            "a dialog's own popup must stay above it"
        );
        assert!(ctx.memory(|memory| memory.allows_interaction(layer)));
        assert_eq!(ctx.layer_id_at(item_rect.center()), Some(layer));
        let mut accepted = false;
        for pressed in [true, false] {
            accepted = frame(&mut app, mask_detect_pointer(item_rect.center(), pressed)).2;
        }
        assert!(
            accepted,
            "menu={menu}: popup item must receive the click above the modal"
        );
        assert_eq!(
            mask_detect_raise_count(&ctx, "mask_detect_settings"),
            raises_before_popup,
            "do not chase the dialog's own popup"
        );
    }
}

#[test]
fn mask_detect_settings_escape_only_closes_front_real_modal_in_either_draw_order() {
    let _input = crate::key_input::lock_test_input();
    for upper_first in [false, true] {
        let mut app = mask_detect_fixture(MaskDetectTool::Erase);
        app.mask_detect_dialog = Some(MaskDetectTool::Erase);
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let own = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("mask_detect_settings"),
        );
        let upper = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("external_tool_preparing"),
        );
        let show_upper = |open: &mut bool| {
            if !*open {
                return false;
            }
            // Same Modal id and pure body as the external-tool dialog, without
            // creating its operation/worker or invoking an external process.
            let response = egui::Modal::new(upper.id).show(&ctx, |ui| {
                ui.set_min_width(380.0);
                crate::external_tool::draw_external_tool_preparing_body(ui, 1)
            });
            let close = response.inner || response.should_close();
            if close {
                *open = false;
            }
            close
        };
        let frame = |app: &mut App, upper_open: &mut bool, events| {
            let mut upper_closed = false;
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(transform().viewport_rect),
                    events,
                    ..Default::default()
                },
                |_| {
                    if upper_first {
                        upper_closed |= show_upper(upper_open);
                    }
                    app.show_mask_detect_dialog(&ctx);
                    if !upper_first {
                        upper_closed |= show_upper(upper_open);
                    }
                },
            )
            .drop_without_applying_deltas();
            upper_closed
        };
        let mut upper_open = false;
        for _ in 0..4 {
            assert!(!frame(&mut app, &mut upper_open, Vec::new()));
        }
        assert_eq!(ctx.memory(|memory| memory.top_modal_layer()), Some(own));
        upper_open = true;
        for _ in 0..4 {
            assert!(!frame(&mut app, &mut upper_open, Vec::new()));
        }
        assert_eq!(ctx.memory(|memory| memory.top_modal_layer()), Some(upper));
        assert!(!ctx.memory(|memory| memory.allows_interaction(own)));
        assert!(frame(
            &mut app,
            &mut upper_open,
            vec![key(egui::Key::Escape)]
        ));
        assert!(!upper_open, "the front Modal must close");
        assert_eq!(
            app.mask_detect_dialog,
            Some(MaskDetectTool::Erase),
            "the back dialog must survive, upper_first={upper_first}"
        );
        assert!(
            !ctx.input(|input| input.key_pressed(egui::Key::Escape)),
            "the front Modal must consume Escape"
        );
        // Release the key and let the standard modal registration return to the
        // settings window before sending a fresh Escape to that window.
        frame(
            &mut app,
            &mut upper_open,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: Some(egui::Key::Escape),
                pressed: false,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert_eq!(ctx.memory(|memory| memory.top_modal_layer()), Some(own));
        frame(&mut app, &mut upper_open, vec![key(egui::Key::Escape)]);
        assert!(app.mask_detect_dialog.is_none());
    }
}

#[test]
fn mask_detect_settings_escape_closes_without_other_modal_on_first_and_later_frames() {
    let _input = crate::key_input::lock_test_input();
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        for warm_frames in [0, 4] {
            let mut app = mask_detect_fixture(tool);
            app.mask_detect_dialog = Some(tool);
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            let frame = |app: &mut App, events| {
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(transform().viewport_rect),
                        events,
                        ..Default::default()
                    },
                    |_| app.show_mask_detect_dialog(&ctx),
                )
                .drop_without_applying_deltas();
            };
            for _ in 0..warm_frames {
                frame(&mut app, Vec::new());
            }
            let expected = (warm_frames != 0).then(|| {
                egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::unique("mask_detect_settings"),
                )
            });
            assert_eq!(ctx.memory(|memory| memory.top_modal_layer()), expected);
            frame(&mut app, vec![key(egui::Key::Escape)]);
            assert!(
                app.mask_detect_dialog.is_none(),
                "{tool:?}, warm_frames={warm_frames}"
            );
        }
    }
}

#[test]
fn mask_detect_foreground_modal_keeps_details_numeric_input_and_ime_active() {
    let _input = crate::key_input::lock_test_input();
    let mut app = mask_detect_fixture(MaskDetectTool::Erase);
    app.mask_detect_dialog = Some(MaskDetectTool::Erase);
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    crate::ime_focus::install_ime_input_policy(&ctx);
    let frame = |app: &mut App, events| {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(transform().viewport_rect),
                events,
                ..Default::default()
            },
            |_| {
                app.update_ime_state(&ctx);
                app.show_mask_detect_dialog(&ctx);
            },
        )
    };
    for _ in 0..4 {
        frame(&mut app, Vec::new()).drop_without_applying_deltas();
    }
    let header = ctx
        .data(|d| d.get_temp::<egui::Rect>(egui::Id::unique("mask_detect_details")))
        .unwrap();
    for pressed in [true, false] {
        frame(&mut app, mask_detect_pointer(header.center(), pressed))
            .drop_without_applying_deltas();
    }
    for _ in 0..12 {
        frame(&mut app, Vec::new()).drop_without_applying_deltas();
    }
    let threshold = ctx
        .data(|d| d.get_temp::<egui::Rect>(egui::Id::unique("mask_detect_threshold_rect")))
        .unwrap();
    // Slider places its DragValue directly after the slider rail, with the
    // current style's item spacing. Clicking that number opens its TextEdit.
    let point = egui::pos2(
        threshold.left()
            + ctx.style_of(egui::Theme::Dark).spacing.slider_width
            + ctx.style_of(egui::Theme::Dark).spacing.item_spacing.x
            + 5.0,
        threshold.center().y,
    );
    for pressed in [true, false] {
        frame(&mut app, mask_detect_pointer(point, pressed)).drop_without_applying_deltas();
    }
    let output = frame(&mut app, Vec::new());
    assert!(
        output.platform_output.ime.is_some(),
        "the real threshold DragValue must be editing text"
    );
    output.drop_without_applying_deltas();
    let focused = ctx
        .memory(|memory| memory.focused())
        .expect("numeric input focus");
    let dialog = egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("mask_detect_settings"),
    );
    assert_eq!(ctx.read_response(focused).unwrap().layer_id, dialog);
    frame(&mut app, vec![egui::Event::Text("0.42".into())]).drop_without_applying_deltas();
    assert!((app.settings.mask_detect.threshold - 0.42).abs() < 0.001);
    // Verify IME composition/commit while the actual numeric editor owns focus.
    // Select the previous number so the committed text replaces the whole value.
    frame(
        &mut app,
        vec![egui::Event::Key {
            key: egui::Key::A,
            physical_key: Some(egui::Key::A),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                ctrl: true,
                command: true,
                ..Default::default()
            },
        }],
    )
    .drop_without_applying_deltas();
    let output = frame(
        &mut app,
        vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: false,
                repeat: false,
                modifiers: Default::default(),
            },
            egui::Event::Ime(egui::ImeEvent::Enabled),
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "0.73".into(),
                active_range_chars: None,
            }),
        ],
    );
    assert!(app.ime_input_active(&ctx));
    assert!(output.platform_output.ime.is_some());
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(focused));
    assert!(ctx.memory(|memory| memory.allows_interaction(dialog)));
    output.drop_without_applying_deltas();
    let output = frame(
        &mut app,
        vec![egui::Event::Ime(egui::ImeEvent::Commit("0.73".into()))],
    );
    assert!(!app.ime_input_active(&ctx));
    assert!((app.settings.mask_detect.threshold - 0.73).abs() < 0.001);
    assert!(output.platform_output.ime.is_some());
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(focused));
    output.drop_without_applying_deltas();
    // A deliberately injected raw Escape is a separate app-dialog guard test.
    // egui Focus::begin_pass clears focus before building the DragValue; its
    // numeric editor intentionally does not restore focus (egui-ime's contract).
    // Do not require IMEOutput or the old field focus after numeric cancellation.
    let output = frame(
        &mut app,
        vec![
            egui::Event::Ime(egui::ImeEvent::Enabled),
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "未確定".into(),
                active_range_chars: None,
            }),
            key(egui::Key::Escape),
        ],
    );
    assert!(app.ime_input_active(&ctx));
    assert!(!app.dialog_escape_pressed(&ctx));
    assert_eq!(app.mask_detect_dialog, Some(MaskDetectTool::Erase));
    output.drop_without_applying_deltas();
    frame(
        &mut app,
        vec![
            egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: Some(egui::Key::Escape),
                pressed: false,
                repeat: false,
                modifiers: Default::default(),
            },
            egui::Event::Ime(egui::ImeEvent::Commit("".into())),
        ],
    )
    .drop_without_applying_deltas();
    assert!(!app.ime_input_active(&ctx));
    assert_eq!(app.mask_detect_dialog, Some(MaskDetectTool::Erase));
    assert!((app.settings.mask_detect.threshold - 0.73).abs() < 0.001);
    // Re-enter through the real number button after Escape, ensuring the modal
    // still permits editing rather than just allowing an earlier focus claim.
    for pressed in [true, false] {
        frame(&mut app, mask_detect_pointer(point, pressed)).drop_without_applying_deltas();
    }
    let output = frame(&mut app, Vec::new());
    assert!(output.platform_output.ime.is_some());
    output.drop_without_applying_deltas();
    frame(&mut app, vec![egui::Event::Text("0.58".into())]).drop_without_applying_deltas();
    assert!((app.settings.mask_detect.threshold - 0.58).abs() < 0.001);
    // Like other dialogs using dialog_escape_pressed, normal Escape is admitted
    // again after the composition has ended.
    frame(&mut app, vec![key(egui::Key::Escape)]).drop_without_applying_deltas();
    assert!(app.mask_detect_dialog.is_none());
}

fn mask_detect_append_receipt(
    app: &crate::app::AppTestEnvForTest,
    operation_id: u64,
) -> crate::book_append_undo::BookAppendUndoReceipt {
    let root = app.tmp.path().join("append-books");
    let folder = root.join("book");
    std::fs::create_dir_all(&folder).unwrap();
    let mut builder =
        crate::book_append_undo::AppendReceiptBuilder::begin(&root, &folder, "book".into())
            .unwrap();
    // An append receipt requires at least one output recorded after begin().
    let added = folder.join("0001_added.png");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([70, 100, 130, 255]))
        .save(&added)
        .unwrap();
    builder.record(&added).unwrap();
    let files = builder.finish().unwrap();
    files.verify_current().unwrap();
    assert_eq!(files.created.len(), 1);
    crate::book_append_undo::BookAppendUndoReceipt {
        files,
        operation_id,
        source_context: app.projected_viewer_context_id(),
        accepted_revision: app.book_append_undo_revision,
    }
}

#[test]
fn mask_detect_modal_blocks_tool_keys_raw_canvas_and_both_local_panels() {
    let _input = crate::key_input::lock_test_input();
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        for pending in [false, true] {
            let mut app = mask_detect_fixture(tool);
            app.erase_tool = EraseTool::Brush;
            app.conceal_tool = crate::conceal::ConcealTool::Brush;
            app.local_adjust_mask_tool = LocalAdjustMaskTool::Brush;
            if pending {
                let request = app.capture_mask_detect_request(tool).unwrap();
                mask_detect_queue(&mut app, request, mask_detect_output());
            } else {
                app.open_mask_detect_dialog(tool);
            }
            let before = app.local_adjust_page_layers.get(&0).cloned();
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            let transform = transform();
            for pressed in [true, false] {
                ctx.run_ui(
                    egui::RawInput {
                        focused: true,
                        screen_rect: Some(transform.viewport_rect),
                        events: vec![
                            key(egui::Key::K),
                            key(egui::Key::Escape),
                            egui::Event::PointerMoved(egui::pos2(680.0, 280.0)),
                            egui::Event::PointerButton {
                                pos: egui::pos2(680.0, 280.0),
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: Default::default(),
                            },
                        ],
                        ..Default::default()
                    },
                    |ui| {
                        app.keyboard_owner_for_pass(&ctx);
                        match tool {
                            MaskDetectTool::Erase => {
                                app.handle_erase_keys(&ctx, 0);
                                app.handle_erase_paint(&ctx, &transform, Default::default());
                                app.draw_erase_overlay(ui, &ctx, &transform);
                            }
                            MaskDetectTool::Conceal => {
                                app.handle_conceal_keys(&ctx, 0);
                                app.handle_conceal_paint(&ctx, &transform, Default::default());
                                app.draw_conceal_panel(&ctx, transform.viewport_rect);
                            }
                            MaskDetectTool::LocalAdjust => {
                                app.handle_fs_key_input(&ctx, 0, false);
                                app.handle_local_adjust_canvas_input(
                                    &ctx,
                                    transform.viewport_rect,
                                    &transform,
                                    Default::default(),
                                );
                                app.draw_local_adjust_panel(&ctx, transform.viewport_rect);
                            }
                        }
                        assert!(
                            ctx.input(|i| i.events.iter().any(|e| matches!(
                                e,
                                egui::Event::Key {
                                    key: egui::Key::K,
                                    ..
                                }
                            ))),
                            "modal must not consume LaToolBucket or other tool keys"
                        );
                    },
                )
                .drop_without_applying_deltas();
            }
            let panel_ids: &[&str] = match tool {
                MaskDetectTool::Erase => &["mask_detect_erase_panel_enabled"],
                MaskDetectTool::Conceal => &["mask_detect_conceal_panel_enabled"],
                MaskDetectTool::LocalAdjust => &[
                    "mask_detect_local_left_panel_enabled",
                    "mask_detect_local_right_panel_enabled",
                ],
            };
            for id in panel_ids {
                assert_eq!(
                    ctx.data(|d| d.get_temp::<bool>(egui::Id::unique(id))),
                    Some(false),
                    "panel {id} must be disabled"
                );
            }
            assert_eq!(app.erase_tool, EraseTool::Brush);
            assert_eq!(app.conceal_tool, crate::conceal::ConcealTool::Brush);
            assert_eq!(app.local_adjust_mask_tool, LocalAdjustMaskTool::Brush);
            assert!(app.erase_mask.as_ref().unwrap().iter().all(|v| !v));
            assert!(app.conceal_mask.as_ref().unwrap().iter().all(|v| !v));
            assert!(app.erase_shapes.is_empty() && app.conceal_shapes.is_empty());
            assert_eq!(app.local_adjust_page_layers.get(&0), before.as_ref());
            assert!(
                app.mask_detect_active(),
                "mode-exit key must not dismiss detection"
            );
        }
    }
}

#[test]
fn mask_detect_eye_preview_rejects_start_but_keeps_existing_hold_until_release() {
    let _input = crate::key_input::lock_test_input();
    for tool in [MaskDetectTool::Erase, MaskDetectTool::Conceal] {
        for active_before in [false, true] {
            let mut app = mask_detect_fixture(tool);
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            let transform = transform();
            // A ready cached erase preview avoids invoking inference in this input test.
            let pixels = Arc::clone(&app.erase_base_cache[&0].pixels);
            let basis = app.erase_base_cache[&0].display_basis.clone();
            app.erase_preview_cache.insert(
                0,
                ErasePreviewCacheEntry {
                    display_basis: basis,
                    pixels: Arc::clone(&pixels),
                    texture: ctx.load_texture(
                        "held-preview",
                        pixels.as_ref().clone(),
                        Default::default(),
                    ),
                },
            );
            let frame = |app: &mut App, events: Vec<egui::Event>| {
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(transform.viewport_rect),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        app.keyboard_owner_for_pass(&ctx);
                        if tool == MaskDetectTool::Conceal {
                            app.sync_conceal_preview_before_pipeline(&ctx, egui::ViewportId::ROOT);
                            app.draw_conceal_panel(&ctx, transform.viewport_rect);
                        } else {
                            app.handle_erase_paint(&ctx, &transform, Default::default());
                            app.draw_erase_overlay(ui, &ctx, &transform);
                        }
                    },
                )
                .drop_without_applying_deltas();
            };
            for _ in 0..4 {
                frame(&mut app, Vec::new());
            }
            let eye = if tool == MaskDetectTool::Conceal {
                app.conceal_preview_button_rect.unwrap()
            } else {
                ctx.data(|d| {
                    d.get_temp::<egui::Rect>(egui::Id::unique("mask_detect_erase_eye_rect"))
                })
                .unwrap()
            };
            let pointer = |pressed| {
                vec![
                    egui::Event::PointerMoved(eye.center()),
                    egui::Event::PointerButton {
                        pos: eye.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ]
            };
            if active_before {
                frame(&mut app, pointer(true));
                assert!(if tool == MaskDetectTool::Conceal {
                    app.conceal_preview_active
                } else {
                    app.erase_preview_active
                });
            }
            app.open_mask_detect_dialog(tool);
            assert!(
                app.mask_detect_active(),
                "fixture must enter the detection modal: {tool:?}"
            );
            if active_before {
                // The eye owns an accepted press through movement outside its rect.
                frame(
                    &mut app,
                    vec![egui::Event::PointerMoved(
                        eye.center() + egui::vec2(300.0, 100.0),
                    )],
                );
            } else {
                frame(&mut app, pointer(true));
            }
            assert_eq!(
                if tool == MaskDetectTool::Conceal {
                    app.conceal_preview_active
                } else {
                    app.erase_preview_active
                },
                active_before
            );
            frame(&mut app, pointer(false));
            assert!(!app.conceal_preview_active && !app.erase_preview_active);
            assert!(app.erase_inpaint_pending.is_empty());
        }
    }
}

#[test]
fn mask_detect_mode_exit_and_index_invalidation_cancel_only_owned_request() {
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        for indices_changed in [false, true] {
            let mut app = mask_detect_fixture(tool);
            let request = app.capture_mask_detect_request(tool).unwrap();
            let cancel = mask_detect_queue(&mut app, request, mask_detect_output());
            if indices_changed {
                app.invalidate_idx_state_and_queues();
            } else {
                match tool {
                    MaskDetectTool::Erase => app.reset_erase_mode(),
                    MaskDetectTool::Conceal => app.reset_conceal_mode(),
                    MaskDetectTool::LocalAdjust => app.exit_local_adjust_mode(
                        crate::app::page_edit_item_change::LocalAdjustmentExitPreview::CacheCurrent,
                    ),
                }
            }
            assert!(cancel.load(Ordering::Acquire));
            assert!(app.mask_detect_pending.is_none());
            assert!(!app.mask_detect_active());
        }
    }
}

#[test]
fn mask_detect_cancel_keeps_inference_activity_until_worker_really_finishes() {
    let mut app = mask_detect_fixture(MaskDetectTool::Erase);
    let request = app
        .capture_mask_detect_request(MaskDetectTool::Erase)
        .unwrap();
    let (entered, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel::<()>();
    let activity = Arc::clone(&app.local_ai_activity);
    let before = activity.load(Ordering::Acquire);
    app.launch_mask_detect_worker(request, move |_| {
        let _ = entered.send(());
        // Simulate an in-flight synchronous inference that cannot be interrupted.
        let _ = wait.recv();
        Err(MaskDetectError::Detect(DetectError::Cancelled))
    });
    ready
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    app.cancel_mask_detect();
    assert!(!app.mask_detect_active());
    assert_eq!(activity.load(Ordering::Acquire), before + 1);
    assert!(!app.local_ai_remote_barrier_snapshot().is_quiesced());
    drop(release);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while activity.load(Ordering::Acquire) != before {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(app.erase_shapes.is_empty());
}

#[test]
fn mask_detect_remote_and_append_undo_wait_reject_all_three_tools() {
    for remote in [false, true] {
        for tool in [
            MaskDetectTool::Erase,
            MaskDetectTool::Conceal,
            MaskDetectTool::LocalAdjust,
        ] {
            let mut app = mask_detect_fixture(tool);
            if remote {
                let handle = crate::remote_ipc::session::SessionHandle::new();
                app.set_remote_session_handle(handle.clone());
                let _ = handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
                    client_id: "mask-detect-test".into(),
                    peer: mimageviewer_ipc::SessionPeerInfo {
                        connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                        device_name: Some("test".into()),
                    },
                });
                assert!(app.remote_session_blocks_local_control());
            } else {
                let receipt = mask_detect_append_receipt(&app, 1);
                let (_tx, rx) = mpsc::channel();
                app.book_op_pending = Some(crate::books::BookOpPending {
                    rx,
                    shelf_scope: None,
                    intent: crate::books::BookOpIntent::UndoAppend { receipt },
                });
            }
            assert!(!app.mask_detect_available(tool));
            app.open_mask_detect_dialog(tool);
            assert!(!app.mask_detect_active());
            assert!(app.capture_mask_detect_request(tool).is_none());
            assert!(app.erase_shapes.is_empty() && app.conceal_shapes.is_empty());
        }
    }
}

#[test]
fn mask_detect_settings_roundtrip_and_missing_key_defaults_are_shared() {
    let mut settings = crate::settings::Settings::default();
    settings.mask_detect.ellipse = false;
    settings.mask_detect.margin_percent = 27.0;
    settings.mask_detect.threshold = 0.7;
    settings.mask_detect.face_selected = false;
    settings.mask_detect.contour_margin_percent = 0.9;
    settings.mask_detect.detectors.insert(
        "remembered".into(),
        crate::settings::ExternalDetectorSettings {
            selected: true,
            classes: Some(vec!["chosen".into()]),
            shape: Some(DetectorShape::Contour),
            threshold: Some(0.35),
        },
    );
    let mut stored = serde_json::to_value(&settings).unwrap();
    let restored: crate::settings::Settings = serde_json::from_value(stored.clone()).unwrap();
    assert!(!restored.mask_detect.ellipse);
    assert_eq!(restored.mask_detect.margin_percent, 27.0);
    assert_eq!(restored.mask_detect.threshold, 0.7);
    assert!(!restored.mask_detect.face_selected);
    assert_eq!(restored.mask_detect.contour_margin_percent, 0.9);
    let detector = &restored.mask_detect.detectors["remembered"];
    assert!(detector.selected);
    assert_eq!(
        detector.classes.as_deref(),
        Some(["chosen".into()].as_slice())
    );
    assert_eq!(detector.shape, Some(DetectorShape::Contour));
    assert_eq!(detector.threshold, Some(0.35));
    let missing: crate::settings::MaskDetectSettings =
        serde_json::from_str(r#"{"detectors":{"remembered":{"selected":true}}}"#).unwrap();
    assert!(missing.face_selected);
    assert_eq!(missing.contour_margin_percent, 0.6);
    assert!(missing.detectors["remembered"].classes.is_none());
    assert!(missing.detectors["remembered"].shape.is_none());
    assert!(missing.detectors["remembered"].threshold.is_none());
    stored.as_object_mut().unwrap().remove("mask_detect");
    let defaults: crate::settings::Settings = serde_json::from_value(stored).unwrap();
    assert!(defaults.mask_detect.ellipse);
    assert_eq!(defaults.mask_detect.margin_percent, 15.0);
    assert_eq!(defaults.mask_detect.threshold, 0.6);
    assert!(defaults.mask_detect.face_selected);
    assert_eq!(defaults.mask_detect.contour_margin_percent, 0.6);
    assert!(defaults.mask_detect.detectors.is_empty());
}

#[test]
fn mask_detect_margin_geometry_and_paint_operation_are_independent_of_tool_erase_toggle() {
    for ellipse in [false, true] {
        let options = crate::settings::MaskDetectSettings {
            ellipse,
            ..Default::default()
        };
        let shape = mask_detect_boxes(
            vec![Detection {
                class_idx: 0,
                score: 0.9,
                rect: [2.0, 2.0, 6.0, 6.0],
            }],
            &options,
        )
        .boxes
        .pop()
        .unwrap()
        .into_shape();
        assert_eq!(shape.center(), (4.0, 4.0));
        match shape {
            crate::mask_db::Shape::Rect {
                half_w,
                half_h,
                rotation_rad,
                ..
            } => {
                assert!(!ellipse);
                assert!((half_w - 2.6).abs() < 0.0001 && (half_h - 2.6).abs() < 0.0001);
                assert_eq!(rotation_rad, 0.0);
            }
            crate::mask_db::Shape::Ellipse {
                rx,
                ry,
                rotation_rad,
                ..
            } => {
                assert!(ellipse);
                assert!((rx - 2.6).abs() < 0.0001 && (ry - 2.6).abs() < 0.0001);
                assert_eq!(rotation_rad, 0.0);
            }
            _ => panic!("box geometry"),
        }
    }
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        let mut app = mask_detect_fixture(tool);
        app.erase_paint_mode = false;
        app.conceal_paint_mode = false;
        app.local_adjust_mask_paint_add = false;
        let request = app.capture_mask_detect_request(tool).unwrap();
        app.apply_mask_detect_output(&request, mask_detect_output());
        match tool {
            MaskDetectTool::Erase => {
                assert_eq!(app.erase_shapes[0].op(), crate::mask_db::ShapeOp::Add)
            }
            MaskDetectTool::Conceal => {
                assert_eq!(app.conceal_shapes[0].op(), crate::mask_db::ShapeOp::Add)
            }
            MaskDetectTool::LocalAdjust => {
                let local_adjust_core::LocalMask::RasterVector(mask) =
                    &app.local_adjust_page_layers[&0][0].mask
                else {
                    panic!("vector target");
                };
                assert_eq!(mask.shapes[0].op(), local_adjust_core::ShapeOp::Add);
            }
        }
    }
}

#[test]
fn mask_detect_addition_invalidates_only_the_target_page_cache() {
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        let mut app = mask_detect_fixture(tool);
        let request = app.capture_mask_detect_request(tool).unwrap();
        let ctx = egui::Context::default();
        let pixels = Arc::clone(&request.source);
        let basis = crate::material_display_basis::MaterialDisplayBasis::from_sampler(
            [8, 8],
            crate::source_coverage::SourceCoverage::FullSource,
        );
        let local_keys = [
            app.current_local_adjust_key(0),
            app.current_local_adjust_key(1),
        ];
        for idx in [0, 1] {
            match tool {
                MaskDetectTool::Erase => {
                    app.erase_preview_cache.insert(
                        idx,
                        ErasePreviewCacheEntry {
                            display_basis: basis.clone(),
                            pixels: Arc::clone(&pixels),
                            texture: ctx.load_texture(
                                format!("mask-detect-cache-{idx}"),
                                pixels.as_ref().clone(),
                                Default::default(),
                            ),
                        },
                    );
                }
                MaskDetectTool::Conceal => {
                    let generation = app.conceal_generation;
                    app.conceal_cache.insert(
                        idx,
                        ConcealCacheEntry {
                            texture_options: Default::default(),
                            display_basis: basis.clone(),
                            pixels: Arc::clone(&pixels),
                            texture: None,
                            generation,
                        },
                    );
                }
                MaskDetectTool::LocalAdjust => {
                    app.local_adjust_cache.insert(
                        local_keys[idx],
                        LocalAdjustCacheEntry {
                            texture_options: Default::default(),
                            display_basis: basis.clone(),
                            pixels: Arc::clone(&pixels),
                            texture: None,
                        },
                    );
                }
            }
        }
        let generation = app.conceal_generation;
        app.apply_mask_detect_output(&request, mask_detect_output());
        match tool {
            MaskDetectTool::Erase => assert!(
                !app.erase_preview_cache.contains_key(&0)
                    && app.erase_preview_cache.contains_key(&1)
            ),
            MaskDetectTool::Conceal => {
                assert!(!app.conceal_cache.contains_key(&0) && app.conceal_cache.contains_key(&1))
            }
            MaskDetectTool::LocalAdjust => assert!(
                !app.local_adjust_cache.contains_key(&local_keys[0])
                    && app.local_adjust_cache.contains_key(&local_keys[1])
            ),
        }
        assert_eq!(app.conceal_generation, generation, "no global invalidation");
    }
}

#[test]
fn mask_detect_open_cancel_and_zero_keep_book_append_undo_receipt() {
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        for zero in [false, true] {
            let mut app = mask_detect_fixture(tool);
            let revision = app.book_append_undo_revision;
            app.book_append_undo_receipt = Some(mask_detect_append_receipt(&app, 7));
            app.open_mask_detect_dialog(tool);
            assert!(app.book_append_undo_receipt.is_some());
            let request = app.capture_mask_detect_request(tool).unwrap();
            if zero {
                mask_detect_queue(
                    &mut app,
                    request,
                    mask_detect_boxes(Vec::new(), &Default::default()),
                );
                app.poll_mask_detect(&egui::Context::default());
            } else {
                mask_detect_queue(&mut app, request, mask_detect_output());
                app.cancel_mask_detect();
            }
            assert_eq!(app.book_append_undo_revision, revision);
            assert_eq!(
                app.book_append_undo_receipt.as_ref().unwrap().operation_id,
                7
            );
        }
    }
}

#[test]
fn mask_detect_expanded_dialog_keeps_actions_outside_scroll_at_small_screen() {
    use crate::test_ui::scroll_layout::{HeightExpectation, ScrollLayout};
    let _input = crate::key_input::lock_test_input();
    // With no added models, the contour margin is hidden. The folder controls
    // and license notice are still part of the expanded body.
    // With details expanded they fit at 100%, but require scrolling at 200%.
    for (scale, expected_height) in [
        (1.0_f32, HeightExpectation::Fits),
        (2.0, HeightExpectation::Scrolls),
    ] {
        let mut app = mask_detect_fixture(MaskDetectTool::Erase);
        app.open_mask_detect_dialog(MaskDetectTool::Erase);
        // Exercise the normal empty catalog, independently of worker timing or
        // the fixture's disconnected receiver and its error-message layout.
        *app.mask_detect_catalog = DetectorCatalogState::Ready(Ok(Vec::new()));
        assert_eq!(
            mask_detect_margin_visibility(&app.mask_detect_catalog, &app.settings.mask_detect),
            (true, false)
        );
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        ctx.set_pixels_per_point(scale);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1093.0, 614.0) / scale);
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app.show_mask_detect_dialog(ui.ctx());
                },
            )
            .drop_without_applying_deltas();
        };
        for _ in 0..4 {
            frame(&mut app, Vec::new());
        }
        let header = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::unique("mask_detect_details")))
            .unwrap();
        for pressed in [true, false] {
            frame(
                &mut app,
                vec![
                    egui::Event::PointerMoved(header.center()),
                    egui::Event::PointerButton {
                        pos: header.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        for _ in 0..12 {
            frame(&mut app, Vec::new());
        }
        assert_eq!(
            ctx.data(|d| d.get_temp::<bool>(egui::Id::unique("mask_detect_details_open"))),
            Some(true)
        );
        let body = ctx
            .data(|d| d.get_temp::<ScrollLayout>(egui::Id::unique("mask_detect_body_layout")))
            .unwrap();
        body.assert_height(expected_height, screen, scale);
        let actions = ctx
            .data(|d| d.get_temp::<Vec<egui::Rect>>(egui::Id::unique("mask_detect_actions")))
            .unwrap();
        let window = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::unique("mask_detect_window")))
            .unwrap();
        assert!(
            screen.expand(1.0 / scale).contains_rect(window),
            "scale={scale}, window={window:?}, screen={screen:?}"
        );
        assert_eq!(actions.len(), 2);
        for action in actions {
            assert!(
                screen.contains_rect(action),
                "scale={scale}, action={action:?}, screen={screen:?}"
            );
            assert!(
                window.contains_rect(action),
                "scale={scale}, action={action:?}, window={window:?}"
            );
            assert!(
                action.top() >= body.inner.bottom(),
                "scale={scale}, action={action:?}, allocated_body={body:?}"
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn mask_detect_parked_context_poll_and_close_leave_sibling_masks_unchanged() {
    for close in [false, true] {
        // Local documents really are bundle-owned. Erase/conceal drafts are
        // foreground scratch, and their real handoff is covered separately.
        let mut app = mask_detect_fixture(MaskDetectTool::LocalAdjust);
        let request = app
            .capture_mask_detect_request(MaskDetectTool::LocalAdjust)
            .unwrap();
        let owner_key = request.page_key.clone();
        let cancel = mask_detect_queue(&mut app, request, mask_detect_output());
        let owner = app.stash_mounted_and_start_fresh("mask_detect_test");
        let sibling = app.projected_viewer_context_id();
        assert_ne!(owner, sibling);
        let ctx = egui::Context::default();
        let (sibling_layers, sibling_cache_key, sibling_pixels) =
            mask_detect_install_sibling_local(&mut app, &ctx);
        let undo_before = app.meta_undo.undo_len();
        if close {
            app.retire_context(owner, "mask_detect_test", |_| ())
                .unwrap();
            assert!(cancel.load(Ordering::Acquire));
            assert!(app.with_owner_viewer_context(owner, |_| ()).is_none());
        } else {
            app.poll_all_page_edit_producers(&ctx);
            app.with_owner_viewer_context(owner, |owner_app| {
                assert!(owner_app.mask_detect_pending.is_none());
                let local_adjust_core::LocalMask::RasterVector(mask) =
                    &owner_app.local_adjust_page_layers[&0][0].mask
                else {
                    panic!("vector target");
                };
                assert_eq!(mask.shapes.len(), 1);
            })
            .unwrap();
        }
        app.poll_all_page_edit_producers(&ctx);
        assert_eq!(app.projected_viewer_context_id(), sibling);
        assert!(app.mask_detect_pending.is_none());
        assert!(app.erase_shapes.is_empty() && app.conceal_shapes.is_empty());
        assert_eq!(app.local_adjust_page_layers[&0], sibling_layers);
        assert!(Arc::ptr_eq(
            &app.local_adjust_cache[&sibling_cache_key].pixels,
            &sibling_pixels
        ));
        assert_eq!(app.meta_undo.undo_len(), undo_before + usize::from(!close));
        if !close {
            mask_detect_assert_local_undo_target(&app, &owner_key);
        }
    }
}

#[cfg(windows)]
fn mask_detect_install_sibling_local(
    app: &mut crate::app::AppTestEnvForTest,
    ctx: &egui::Context,
) -> (
    local_adjust_core::LocalAdjustmentLayers,
    LocalAdjustResultKey,
    Arc<egui::ColorImage>,
) {
    let path = app.tmp.path().join("mask-detect-sibling.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([180, 90, 40, 255]))
        .save(&path)
        .unwrap();
    mask_detect_install_source(app, path, MaskDetectTool::LocalAdjust, ctx);
    let mut layers = app.local_adjust_page_layers[&0].clone();
    let layer = &mut Arc::make_mut(&mut layers)[0];
    layer.name = "unchanged sibling".into();
    let local_adjust_core::LocalMask::RasterVector(mask) = &mut layer.mask else {
        panic!("vector target");
    };
    mask.alpha_mut()[0] = 1.0;
    app.set_local_adjust_layers_for_idx_memory_only(0, layers.clone());
    let key = app.current_local_adjust_key(0);
    let pixels = app.current_local_adjust_source_pixels(0).unwrap();
    app.local_adjust_cache.insert(
        key,
        LocalAdjustCacheEntry {
            texture_options: Default::default(),
            display_basis: crate::material_display_basis::MaterialDisplayBasis::from_sampler(
                [8, 8],
                crate::source_coverage::SourceCoverage::FullSource,
            ),
            pixels: Arc::clone(&pixels),
            texture: None,
        },
    );
    (layers, key, pixels)
}

#[cfg(windows)]
fn mask_detect_assert_local_undo_target(app: &App, page_key: &str) {
    // The metadata Undo stack is shared, so assert the captured target rather
    // than incorrectly treating its length as sibling-owned state.
    let Some(crate::undo_stack::UndoEntry::LocalAdjustment { changes, .. }) =
        app.meta_undo.peek_undo()
    else {
        panic!("local detection Undo");
    };
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].target.page_key, page_key);
}

#[cfg(windows)]
#[test]
fn mask_detect_parked_empty_receiver_keeps_root_poll_awake_until_result() {
    let mut app = mask_detect_fixture(MaskDetectTool::LocalAdjust);
    let request = app
        .capture_mask_detect_request(MaskDetectTool::LocalAdjust)
        .unwrap();
    let owner_key = request.page_key.clone();
    let (tx, rx) = mpsc::channel();
    app.mask_detect_pending = Some(Box::new(MaskDetectPending {
        request,
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
    }));
    let owner = app.stash_mounted_and_start_fresh("mask_detect_wait_test");
    let sibling = app.projected_viewer_context_id();
    let ctx = egui::Context::default();
    let (sibling_layers, sibling_cache_key, sibling_pixels) =
        mask_detect_install_sibling_local(&mut app, &ctx);
    let undo_before = app.meta_undo.undo_len();
    for _ in 0..4 {
        crate::test_ui::headless_paint_output(ctx.run_pass(Default::default(), |_| {}))
            .drop_without_applying_deltas();
    }
    let output = crate::test_ui::headless_paint_output(ctx.run_pass(Default::default(), |ctx| {
        app.poll_all_page_edit_producers(ctx);
    }));
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
            <= std::time::Duration::from_millis(100)
    );
    output.drop_without_applying_deltas();
    tx.send(Ok(mask_detect_output())).ok().unwrap();
    app.poll_all_page_edit_producers(&ctx);
    app.with_owner_viewer_context(owner, |owner_app| {
        assert!(owner_app.mask_detect_pending.is_none());
        let local_adjust_core::LocalMask::RasterVector(mask) =
            &owner_app.local_adjust_page_layers[&0][0].mask
        else {
            panic!("vector target");
        };
        assert_eq!(mask.shapes.len(), 1);
    })
    .unwrap();
    assert_eq!(app.projected_viewer_context_id(), sibling);
    assert!(app.mask_detect_pending.is_none());
    assert_eq!(app.local_adjust_page_layers[&0], sibling_layers);
    assert!(Arc::ptr_eq(
        &app.local_adjust_cache[&sibling_cache_key].pixels,
        &sibling_pixels
    ));
    assert_eq!(app.meta_undo.undo_len(), undo_before + 1);
    mask_detect_assert_local_undo_target(&app, &owner_key);
}
