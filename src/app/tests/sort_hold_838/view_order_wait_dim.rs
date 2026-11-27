//! §1.911 stage 6c: the real main grid keeps its opacity only during sort grace.
use super::*;
use crate::filename_stack_ui::{DIRECT_NAVIGATION_WAIT_GRACE, GridItemInputBlock};

fn seed_images(h: &mut Harness) -> Vec<(usize, egui::TextureId)> {
    let mut images = Vec::new();
    let indices: Vec<_> = h
        .env
        .items
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| matches!(item, GridItem::Image(_)).then_some(idx))
        .collect();
    for idx in indices {
        let tex = h.ctx.load_texture(
            format!("view-order-wait-dim-{idx}"),
            egui::ColorImage::filled([8, 6], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        images.push((idx, tex.id()));
        h.env.thumbnails[idx] = ThumbnailState::loaded_raster(
            tex,
            crate::thumb_loader::ThumbLoadOrigin::FinalCache,
            false,
            128,
            crate::source_coverage::SourceCoverage::FullSource,
            Some((8, 6)),
            None,
        );
    }
    assert_eq!(images.len(), 2);
    images
}

/// Draw the production CentralPanel, cell painter and input handler at an exact
/// grace boundary. No App update/poll can consume the held worker reply.
fn draw_at(h: &mut Harness, now: Instant, events: Vec<egui::Event>) -> egui::FullOutput {
    crate::test_ui::headless_paint_output(h.ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 900.0),
            )),
            focused: true,
            events,
            ..Default::default()
        },
        |ctx| {
            ctx.root_ui(|ui| {
                assert!(h.env.render_grid_at(ui, now).is_none());
            });
        },
    ))
}

fn colors(shape: &egui::epaint::Shape, texture: egui::TextureId, out: &mut Vec<egui::Color32>) {
    match shape {
        egui::epaint::Shape::Mesh(mesh) if mesh.texture_id == texture => {
            out.extend(mesh.vertices.iter().map(|vertex| vertex.color));
        }
        egui::epaint::Shape::Vec(shapes) => {
            for shape in shapes {
                colors(shape, texture, out);
            }
        }
        _ => {}
    }
}

fn assert_images(output: &egui::FullOutput, images: &[(usize, egui::TextureId)], alpha: f32) {
    let expected = egui::Color32::WHITE.gamma_multiply(alpha);
    for &(idx, texture) in images {
        let mut painted = Vec::new();
        for clipped in &output.shapes {
            colors(&clipped.shape, texture, &mut painted);
        }
        assert!(
            !painted.is_empty(),
            "cell {idx} must actually paint its image"
        );
        assert!(
            painted.iter().all(|color| *color == expected),
            "cell {idx}: {painted:?}, expected {expected:?}"
        );
    }
}

fn cell_response(h: &Harness, idx: usize) -> egui::Response {
    h.ctx
        .data(|data| {
            data.get_temp(egui::Id::unique((
                "grid_cell_input_response",
                h.env.grid_scroll_context_id(),
                idx,
            )))
        })
        .expect("real grid cell response")
}

fn click_at(h: &mut Harness, now: Instant, idx: usize) {
    let pos = cell_response(h, idx).rect.center();
    draw_at(h, now, vec![egui::Event::PointerMoved(pos)]).drop_without_applying_deltas();
    for pressed in [true, false] {
        draw_at(
            h,
            now,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }],
        )
        .drop_without_applying_deltas();
    }
}

#[test]
fn view_order_wait_dim_grace_keeps_images_opaque_and_blocks_cell_input() {
    let mut h = Harness::images();
    let images = seed_images(&mut h);
    let started = Instant::now();
    // Positive control: these same cells really draw and accept input normally.
    let output = draw_at(&mut h, started, vec![]);
    assert_images(&output, &images, 1.0);
    output.drop_without_applying_deltas();
    assert!(cell_response(&h, 1).enabled());
    click_at(&mut h, started, 1);
    assert_eq!(h.env.selected, Some(1));
    h.env.move_grid_cursor(Some(0), GridCursorIntent::Automatic);

    let _held = request_sort(&mut h.env, SortOrder::FileName);
    h.env.folder_pane_open_pending.as_mut().unwrap().started_at = started;
    for elapsed in [
        Duration::ZERO,
        DIRECT_NAVIGATION_WAIT_GRACE - Duration::from_millis(1),
    ] {
        let now = started + elapsed;
        assert!(h.env.direct_navigation_wait_overlay_delay_at(now).is_some());
        assert_eq!(
            h.env.grid_item_input_block(),
            Some(GridItemInputBlock::ViewOrderRefreshScan)
        );
        let output = draw_at(&mut h, now, vec![]);
        assert_images(&output, &images, 1.0);
        output.drop_without_applying_deltas();
        for &(idx, _) in &images {
            assert!(!cell_response(&h, idx).enabled());
        }
        click_at(&mut h, now, 1);
        assert_eq!(h.env.selected, Some(0));
        assert!(!h.env.grid_item_input_allowed());
    }
}

#[test]
fn view_order_wait_dim_after_grace_restores_existing_dimmed_images() {
    let mut h = Harness::images();
    let images = seed_images(&mut h);
    let _held = request_sort(&mut h.env, SortOrder::FileName);
    let started = h.env.folder_pane_open_pending.as_ref().unwrap().started_at;
    let alpha = h.ctx.global_style().visuals.disabled_alpha;
    assert!(alpha > 0.0 && alpha < 1.0);
    for elapsed in [
        DIRECT_NAVIGATION_WAIT_GRACE,
        DIRECT_NAVIGATION_WAIT_GRACE + Duration::from_millis(1),
    ] {
        let now = started + elapsed;
        assert!(h.env.direct_navigation_wait_overlay_delay_at(now).is_none());
        // The overlay Area may need its first sizing pass.
        draw_at(&mut h, now, vec![]).drop_without_applying_deltas();
        let output = draw_at(&mut h, now, vec![]);
        assert_images(&output, &images, alpha);
        let mut text = Vec::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut text);
        }
        assert!(text.iter().any(|text| text == "並べ替え中…"), "{text:?}");
        output.drop_without_applying_deltas();
        assert!(!h.env.grid_item_input_allowed());
        assert!(!cell_response(&h, 0).enabled());
    }
}

#[test]
fn view_order_wait_dim_direct_navigation_still_dims_during_and_after_grace() {
    let mut h = Harness::images();
    let images = seed_images(&mut h);
    let destination = h.env.tmp.path().join("view-order-wait-dim-destination");
    std::fs::create_dir(&destination).unwrap();
    h.env.open_direct_navigation_target_at_stage(
        destination,
        None,
        OpenRequestOwner::Navigation,
        None,
        None,
        StartupListIntent::ExplicitList,
        MainListRequestStage::Admission,
        DirectoryScanRoute::MainWorker,
    );
    let _held = HeldScan::capture(&mut h.env);
    let started = h.env.folder_pane_open_pending.as_ref().unwrap().started_at;
    let alpha = h.ctx.global_style().visuals.disabled_alpha;
    assert!(alpha > 0.0 && alpha < 1.0);
    for elapsed in [Duration::ZERO, DIRECT_NAVIGATION_WAIT_GRACE] {
        let now = started + elapsed;
        assert_eq!(
            h.env.direct_navigation_wait_overlay_delay_at(now).is_some(),
            elapsed < DIRECT_NAVIGATION_WAIT_GRACE
        );
        assert_eq!(
            h.env.grid_item_input_block(),
            Some(GridItemInputBlock::DirectNavigationScan)
        );
        let output = draw_at(&mut h, now, vec![]);
        assert_images(&output, &images, alpha);
        output.drop_without_applying_deltas();
        assert!(!h.env.grid_item_input_allowed());
        assert!(!cell_response(&h, 0).enabled());
    }
}

#[test]
fn view_order_wait_dim_grace_preserves_child_disabled_appearance() {
    let mut h = Harness::images();
    let _held = request_sort(&mut h.env, SortOrder::FileName);
    let now = h.env.folder_pane_open_pending.as_ref().unwrap().started_at;
    let mut checked = false;
    let output = crate::test_ui::headless_paint_output(h.ctx.run_pass(Default::default(), |ctx| {
        ctx.root_ui(|root_ui| {
            egui::CentralPanel::default().show(root_ui, |ui| {
                let opacity = ui.opacity();
                let alpha = ui.visuals().disabled_alpha;
                assert!(opacity > 0.0 && alpha > 0.0 && alpha < 1.0);
                h.env.gate_main_grid_item_input_at(ui, now);
                assert!(!ui.is_enabled());
                assert_eq!(ui.opacity(), opacity);
                assert_eq!(ui.visuals().disabled_alpha, alpha);
                ui.add_enabled_ui(false, |ui| {
                    assert!(!ui.is_enabled());
                    assert_eq!(ui.opacity(), opacity * alpha);
                    checked = true;
                });
            });
        });
    }));
    output.drop_without_applying_deltas();
    assert!(checked);
}
