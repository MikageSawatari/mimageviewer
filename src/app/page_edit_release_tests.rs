//! Actual pointer-release parity with the pre-owner terminals in a5648d3cb.
#![cfg(test)]

use super::tests::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use crate::displayed_image_transform::{DisplayedImageTransform, DisplayedImageTransformInput};
use crate::undo_stack::{LocalAdjustmentChange, UndoEntry};
use local_adjust_core::{LocalAdjustmentLayer, LocalAdjustmentLayers, LocalEffect, LocalMask};

#[derive(Clone, Copy)]
enum PointerEdit {
    Shape,
    Canvas,
    Brush,
}

impl PointerEdit {
    fn summary(self) -> &'static str {
        match self {
            Self::Shape => "補正レイヤー図形マスク編集",
            Self::Canvas => "補正レイヤーキャンバス操作",
            Self::Brush => "補正レイヤー手描きマスク",
        }
    }
}

fn document(name: &str) -> LocalAdjustmentLayers {
    let mut mask = local_adjust_core::RasterVectorMask::empty(1, 1);
    mask.shapes.push(local_adjust_core::MaskShape::Ellipse {
        op: local_adjust_core::ShapeOp::Add,
        center: [0.5, 0.5],
        rx: 0.125,
        ry: 0.125,
        rotation_rad: 0.0,
    });
    LocalAdjustmentLayers::new(vec![LocalAdjustmentLayer::new(
        name,
        LocalMask::RasterVector(mask),
        LocalEffect::RadialBlur(Default::default()),
    )])
}

fn pages() -> (AppTestEnv, Vec<PathBuf>) {
    let mut app = setup_app();
    app.settings.sidecar_backup_enabled = true;
    app.spread_mode = crate::settings::SpreadMode::Single;
    let paths = ["a", "b"]
        .map(|name| {
            let folder = app.tmp.path().join(name);
            std::fs::create_dir_all(&folder).unwrap();
            let path = folder.join("page.png");
            image::RgbaImage::from_pixel(1, 1, image::Rgba([90, 100, 110, 255]))
                .save(&path)
                .unwrap();
            path
        })
        .to_vec();
    app.items = paths.iter().cloned().map(GridItem::Image).collect();
    app.thumbnails = vec![ThumbnailState::Pending; 2];
    app.image_metas = vec![None; 2];
    app.visible_indices = vec![0, 1];
    app.fullscreen_idx = Some(0);
    app.selected = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.local_adjust_mode = true;
    app.local_adjust_selected_shape = Some(0);
    app.local_adjust_mask_edit_target = LocalAdjustMaskEditTarget::Base;
    for (idx, name) in ["a", "b"].into_iter().enumerate() {
        app.set_local_adjust_layers_for_idx_memory_only(idx, document(name));
    }
    app.page_edit_snapshot = Some(page_edit_snapshot::PageEditSnapshot {
        order: Arc::new(page_edit_snapshot::PageKeyOrder::from_items(&app.items).unwrap()),
        ..Default::default()
    });
    for (idx, name) in ["a", "b"].into_iter().enumerate() {
        let target = app.page_adjustment_target_for_idx(idx).unwrap();
        app.set_local_adjust_layers_for_target(&target, document(name));
    }
    app.finish_local_adjust_writes();
    (app, paths)
}

fn transform() -> DisplayedImageTransform {
    DisplayedImageTransform::resolve(DisplayedImageTransformInput {
        pixel_fit: crate::displayed_image_transform::RectPixelFit::Texels,
        page_idx: 0,
        viewport_rect: egui::Rect::from_min_size(
            egui::pos2(400.0, 100.0),
            egui::vec2(400.0, 400.0),
        ),
        source_size: egui::vec2(1.0, 1.0),
        texture_size: egui::vec2(1.0, 1.0),
        rotation: crate::rotation_db::Rotation::None,
        free_rotation_rad: 0.0,
        content_bbox: None,
        fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
            crate::settings::FullscreenFitMode::Page,
        ),
        fit_scale_limits: crate::displayed_image_transform::FullscreenFitScaleLimits::default(),
        pixels_per_point: 1.0,
        placement: crate::displayed_image_transform::ResolvedDisplayPlacement::Normal {
            zoom_pan: None,
        },
    })
    .unwrap()
}

fn frame(
    app: &mut App,
    ctx: &egui::Context,
    transform: &DisplayedImageTransform,
    events: Vec<egui::Event>,
) {
    let full_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
    ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(full_rect),
            events,
            ..Default::default()
        },
        |ctx| app.handle_local_adjust_canvas_input(ctx, full_rect, transform, Default::default()),
    )
    .drop_without_applying_deltas();
}

fn seed_pointer(app: &mut App, edit: PointerEdit, before: &LocalAdjustmentLayers) {
    // The regression concerns release ordering. Seed the already-started
    // gesture at its real before, then use the real move and release handler.
    let owner = app.capture_page_edit_owner(0).unwrap();
    match edit {
        PointerEdit::Shape => {
            let LocalMask::RasterVector(mask) = &before[0].mask else {
                unreachable!()
            };
            let local_adjust_core::MaskShape::Ellipse {
                center,
                rx,
                ry,
                rotation_rad,
                ..
            } = mask.shapes[0]
            else {
                unreachable!()
            };
            app.local_adjust_mask_tool = LocalAdjustMaskTool::Select;
            app.local_adjust_shape_drag_before_layers = Some(before.clone());
            app.local_adjust_shape_drag = Some(LocalAdjustMaskShapeDrag {
                owner,
                fs_idx: 0,
                layer_idx: 0,
                target: LocalAdjustMaskEditTarget::Base,
                shape_idx: 0,
                vector_drag: crate::vector_edit::begin_drag(
                    crate::vector_edit::HoverTarget::Body,
                    0,
                    crate::mask_db::Shape::Ellipse {
                        op: crate::mask_db::ShapeOp::Add,
                        center: (center[0], center[1]),
                        rx,
                        ry,
                        rotation_rad,
                    },
                    (0.5, 0.5),
                ),
            });
        }
        PointerEdit::Canvas => {
            app.local_adjust_mask_tool = LocalAdjustMaskTool::Select;
            app.local_adjust_canvas_drag_before_layers = Some(before.clone());
            app.local_adjust_canvas_drag = Some(LocalAdjustCanvasDrag {
                owner,
                fs_idx: 0,
                layer_idx: 0,
                kind: LocalAdjustCanvasDragKind::EffectCenter,
                start: [0.5, 0.5],
            });
        }
        PointerEdit::Brush => {
            app.local_adjust_mask_tool = LocalAdjustMaskTool::Brush;
            app.local_adjust_mask_brush_before_layers = Some(before.clone());
            app.local_adjust_mask_brush_stroke = Some(LocalAdjustMaskBrushStroke {
                owner,
                fs_idx: 0,
                layer_idx: 0,
                target: LocalAdjustMaskEditTarget::Base,
                tool: LocalAdjustMaskTool::Brush,
                paint: true,
                edge_seed: None,
                previous: [0.5, 0.5],
                applied: false,
            });
        }
    }
}

/// Source-derived oracle, not the unadopted §9.2 candidate behavior.
/// a5648d3cb ui_adjustment_panel.rs:12039/12072/12143 routes all three
/// releases through undo_ops.rs:606. That setter first persists the key
/// session from its S0 before to the CURRENT S2 map, then records the pointer
/// from its S1 before to the same S2. Preserve that mixed key after verbatim.
/// Only the old idx address is translated to the new captured target.
fn baseline_release_entries(
    target: &PageAdjustmentTarget,
    s0: &LocalAdjustmentLayers,
    s1: &LocalAdjustmentLayers,
    current: &LocalAdjustmentLayers,
    edit: PointerEdit,
) -> Vec<(Vec<LocalAdjustmentChange>, String)> {
    let mut target = target.clone();
    target.idx_hint = PageIndexHint::Unresolved;
    [(s0, "補正レイヤー図形マスク移動"), (s1, edit.summary())]
        .into_iter()
        .filter(|(before, _)| *before != current)
        .map(|(before, summary)| {
            (
                vec![LocalAdjustmentChange {
                    target: target.clone(),
                    before: before.clone(),
                    after: current.clone(),
                }],
                summary.to_string(),
            )
        })
        .collect()
}

fn entry(entry: &UndoEntry) -> (Vec<LocalAdjustmentChange>, String) {
    let UndoEntry::LocalAdjustment { changes, summary } = entry else {
        panic!("expected local history")
    };
    let mut changes = changes.clone();
    // Undo/Redo refresh this projection hint; it is not part of the captured
    // legacy document/address contract being compared here.
    for change in &mut changes {
        change.target.idx_hint = PageIndexHint::Unresolved;
    }
    (changes, summary.clone())
}

fn assert_saved(app: &App, target: &PageAdjustmentTarget, expected: &LocalAdjustmentLayers) {
    assert_eq!(
        app.local_adjust_db
            .as_ref()
            .unwrap()
            .get_layers(&target.page_key)
            .as_deref(),
        Some(expected.as_slice())
    );
    let (folder, item_key) = target.sidecar_coords.as_ref().unwrap();
    assert_eq!(
        app.sidecars
            .get(folder)
            .unwrap()
            .items()
            .get(item_key)
            .and_then(|item| item.local_adjust_layers.as_ref()),
        Some(expected)
    );
}

fn active_document(app: &App) -> (PageEditOwner, LocalAdjustmentLayers, u64, usize) {
    let LocalPageEditDocument::Editing {
        owner,
        layers,
        revision,
        selected_layer,
    } = &app.local_page_edit_document
    else {
        panic!("A must retain its active document through reindex/hydration")
    };
    (owner.clone(), layers.clone(), *revision, *selected_layer)
}

fn hydrate_reindexed_sibling(app: &mut App, a_idx: usize, b_idx: usize) {
    // Search replacement intentionally drops index maps. The production exact
    // prepare publishes presence badges, and inactive documents remain lazy.
    // Model that prepared result from this fixture's actual disposable DB,
    // then read B through the existing DB hydration path. Never restore B from
    // the expected document or start an editing session for it.
    let active_before = active_document(app);
    let key_before = app
        .local_adjust_shape_key_edit
        .as_ref()
        .unwrap()
        .before
        .clone();
    let key_owner = app
        .local_adjust_shape_key_edit
        .as_ref()
        .unwrap()
        .owner
        .clone();
    assert_eq!(app.meta_undo.undo_len(), 0);
    let keys = (0..app.items.len())
        .map(|idx| app.page_path_key(idx).unwrap())
        .collect::<Vec<_>>();
    let snapshot = page_edit_snapshot::PageEditSnapshot {
        order: Arc::new(page_edit_snapshot::PageKeyOrder::from_items(&app.items).unwrap()),
        local_adjust: app
            .local_adjust_db
            .as_ref()
            .unwrap()
            .load_existing_layer_keys(&keys),
        ..Default::default()
    };
    let projection = snapshot.project_order();
    app.install_prepared_page_edits(page_edit_snapshot::StablePageEditProjection {
        snapshot,
        projection,
        stamp: crate::page_edit_write_epoch::PAGE_EDIT_WRITES.sample(),
    });
    assert!(
        app.local_adjust_pages.contains(&b_idx),
        "actual DB prepare must find B"
    );
    assert!(app.load_local_adjust_layers_for_book_snapshot_sync(b_idx));
    assert_eq!(active_document(app), active_before);
    assert_eq!(app.local_adjust_page_layers[&a_idx], active_before.1);
    let key_after = app.local_adjust_shape_key_edit.as_ref().unwrap();
    assert_eq!(key_after.before, key_before);
    assert_eq!(key_after.owner, key_owner);
    assert_eq!(
        app.meta_undo.undo_len(),
        0,
        "inactive hydration must not commit A"
    );
}

fn release_matches_baseline(edit: PointerEdit, reindex: bool) {
    let (mut app, paths) = pages();
    let target_a = app.page_adjustment_target_for_idx(0).unwrap();
    let target_b = app.page_adjustment_target_for_idx(1).unwrap();
    let s0 = app.local_adjust_page_layers[&0].clone();
    let sibling = app.local_adjust_page_layers[&1].clone();
    let ctx = egui::Context::default();
    let transform = transform();
    let pixels = Arc::new(egui::ColorImage::filled([1, 1], egui::Color32::WHITE));
    let tex = ctx.load_texture(
        "release-parity-source",
        (*pixels).clone(),
        Default::default(),
    );
    app.fs_cache.insert(
        0,
        FsCacheEntry::Static {
            tex,
            pixels,
            source_dims: Some([1, 1]),
            load_seq: 1372,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));
    let s1 = app.local_adjust_page_layers[&0].clone();
    assert_ne!(s0, s1);
    // Prime egui's held-pointer state without invoking the start handler.
    ctx.run_pass(
        egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos: egui::pos2(600.0, 300.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
            ..Default::default()
        },
        |_| {},
    )
    .drop_without_applying_deltas();
    seed_pointer(&mut app, edit, &s1);
    frame(
        &mut app,
        &ctx,
        &transform,
        vec![egui::Event::PointerMoved(egui::pos2(700.0, 350.0))],
    );
    let s2 = app.local_adjust_page_layers[&0].clone();
    assert_ne!(
        s1, s2,
        "the actual pointer-move handler must edit the document"
    );
    assert!(app.local_adjust_shape_key_edit.is_some());
    assert_eq!(app.meta_undo.undo_len(), 0);

    let (a_idx, b_idx) = if reindex {
        let replacement = vec![
            GridItem::Image(paths[1].clone()),
            GridItem::Image(paths[0].clone()),
        ];
        let swap = app.prepare_rating_membership_items_swap(&replacement);
        assert!(swap.is_some());
        app.replace_search_view_items(replacement, vec![None; 2]);
        app.finish_rating_membership_viewer_swap(swap);
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!(app.local_adjust_page_layers[&1], s2);
        hydrate_reindexed_sibling(&mut app, 1, 0);
        assert_eq!(app.local_adjust_page_layers[&0], sibling);
        (1, 0)
    } else {
        (0, 1)
    };
    // Keep the key run deliberately below the idle threshold, including when
    // the test process pauses between fixture/adoption and release.
    app.local_adjust_shape_key_edit
        .as_mut()
        .unwrap()
        .last_edit_at = std::time::Instant::now();
    let expected = baseline_release_entries(&target_a, &s0, &s1, &s2, edit);
    assert_eq!(expected.len(), 2);
    frame(
        &mut app,
        &ctx,
        &transform,
        vec![egui::Event::PointerButton {
            pos: egui::pos2(700.0, 350.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
    );
    assert!(
        app.local_adjust_shape_key_edit.is_none(),
        "release must close the earlier key run"
    );
    assert!(!app.has_pending_local_adjust_edit());
    assert_eq!(app.meta_undo.undo_len(), expected.len());
    assert_eq!(entry(app.meta_undo.peek_undo().unwrap()), expected[1]);
    assert_eq!(app.local_adjust_page_layers[&a_idx], s2);
    assert_eq!(app.local_adjust_page_layers[&b_idx], sibling);

    // Immediate Undo must use the pointer before S1, without a history
    // boundary appending the still-pending key run above it.
    app.apply_meta_undo();
    app.finish_local_adjust_writes();
    assert_eq!(app.local_adjust_page_layers[&a_idx], s1);
    assert_saved(&app, &target_a, &s1);
    assert_saved(&app, &target_b, &sibling);
    assert_eq!(entry(app.meta_undo.peek_undo().unwrap()), expected[0]);
    assert_eq!(entry(app.meta_undo.peek_redo().unwrap()), expected[1]);
    assert!(!app.settle_local_adjust_shape_key_edit());
    app.poll_local_adjust_write_results();
    assert_eq!(app.meta_undo.undo_len(), 1);
    assert_eq!(app.meta_undo.redo_len(), 1);
    app.apply_meta_undo();
    app.finish_local_adjust_writes();
    assert_eq!(app.local_adjust_page_layers[&a_idx], s0);
    assert_saved(&app, &target_a, &s0);
    assert_eq!(entry(app.meta_undo.peek_redo().unwrap()), expected[0]);
    for expected_entry in &expected {
        app.apply_meta_redo();
        app.finish_local_adjust_writes();
        assert_eq!(
            app.local_adjust_page_layers[&a_idx], s2,
            "baseline key Redo also restores the mixed S2 after"
        );
        assert_saved(&app, &target_a, &s2);
        assert_eq!(entry(app.meta_undo.peek_undo().unwrap()), *expected_entry);
        assert!(!app.settle_local_adjust_shape_key_edit());
        assert_eq!(app.local_adjust_page_layers[&b_idx], sibling);
        assert_saved(&app, &target_b, &sibling);
    }
    assert_eq!(app.meta_undo.undo_len(), 2);
    assert_eq!(app.meta_undo.redo_len(), 0);
}

#[test]
fn shape_release_keeps_baseline_key_then_pointer_history() {
    release_matches_baseline(PointerEdit::Shape, false);
}

#[test]
fn canvas_release_keeps_baseline_key_then_pointer_history() {
    release_matches_baseline(PointerEdit::Canvas, false);
}

#[test]
fn brush_release_keeps_baseline_key_then_pointer_history() {
    release_matches_baseline(PointerEdit::Brush, false);
}

#[test]
fn reindexed_shape_release_keeps_baseline_history_and_stable_target() {
    release_matches_baseline(PointerEdit::Shape, true);
}

#[test]
fn reindexed_canvas_release_keeps_baseline_history_and_stable_target() {
    release_matches_baseline(PointerEdit::Canvas, true);
}

#[test]
fn reindexed_brush_release_keeps_baseline_history_and_stable_target() {
    release_matches_baseline(PointerEdit::Brush, true);
}
