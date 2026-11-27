use super::*;
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::settings::SpreadMode;
use local_adjust_core::{LocalAdjustmentLayer, LocalAdjustmentLayers, LocalEffect, LocalMask};

fn fixture() -> AppTestEnv {
    let mut app = setup_app();
    let folder = app.tmp.path().join("adoption");
    std::fs::create_dir_all(&folder).unwrap();
    let paths = (0..3)
        .map(|index| folder.join(format!("{index}.png")))
        .collect::<Vec<_>>();
    for path in &paths {
        image::RgbaImage::from_pixel(4, 3, image::Rgba([80, 90, 100, 255]))
            .save(path)
            .unwrap();
    }
    app.items = paths.into_iter().map(GridItem::Image).collect();
    app.thumbnails = vec![ThumbnailState::Pending; 3];
    app.image_metas = vec![None; 3];
    app.visible_indices = vec![0, 1, 2];
    app.current_folder = Some(folder);
    app.fullscreen_idx = Some(1);
    app.selected = Some(1);
    app.spread_mode = SpreadMode::Single;
    app.settings.sidecar_backup_enabled = false;
    app
}

fn pivot() -> PageEditSpreadPivot {
    PageEditSpreadPivot {
        saved_mode: SpreadMode::Ltr,
        pair: (0, 1),
        navigation_anchor_idx: 0,
    }
}

fn begin_local(app: &mut App) -> LocalAdjustmentLayers {
    let mut mask = local_adjust_core::RasterVectorMask::empty(4, 3);
    mask.shapes.push(local_adjust_core::MaskShape::Ellipse {
        op: local_adjust_core::ShapeOp::Add,
        center: [0.5, 0.5],
        rx: 0.125,
        ry: 0.125,
        rotation_rad: 0.0,
    });
    let layers = LocalAdjustmentLayers::new(vec![LocalAdjustmentLayer::new(
        "original",
        LocalMask::RasterVector(mask),
        LocalEffect::None,
    )]);
    app.set_local_adjust_layers_for_idx_memory_only(1, layers.clone());
    app.local_adjust_mode = true;
    app.local_adjust_selected_shape = Some(0);
    app.local_adjust_mask_edit_target = LocalAdjustMaskEditTarget::Base;
    app.select_local_page_edit_layer(1, 0);
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(1, 0.125, 0.0));
    layers
}

fn switched_spread_fixture(
    ctx: &egui::Context,
) -> (AppTestEnv, String, String, LocalAdjustmentLayers) {
    let mut app = fixture();
    begin_local(&mut app);
    let a_key = app.page_path_key(1).unwrap();
    let b_key = app.page_path_key(0).unwrap();
    let a_draft = app.local_adjust_page_layers[&1].clone();
    app.local_adjust_spread_ctx = app.capture_local_page_edit_pivot(pivot());
    for idx in [0, 1] {
        let pixels = Arc::new(egui::ColorImage::filled([4, 3], egui::Color32::WHITE));
        let tex = ctx.load_texture(
            format!("spread-side-{idx}"),
            pixels.clone(),
            Default::default(),
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                pdf_rerender_failures: Vec::new(),
                pdf_metadata: None,
                tex,
                pixels,
                source_dims: Some([4, 3]),
                load_seq: 0,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
    }
    app.switch_local_adjust_target_in_spread(ctx, 0);
    assert_eq!(
        app.fullscreen_idx,
        Some(0),
        "the real spread control must switch A to B"
    );
    assert!(app.local_adjust_mode);
    assert!(
        !app.has_pending_local_adjust_edit(),
        "A must be committed before the switch"
    );
    let retained_owner = match &app.local_page_edit_document {
        LocalPageEditDocument::Selection { owner, .. }
        | LocalPageEditDocument::Editing { owner, .. } => owner,
        LocalPageEditDocument::Idle => panic!("expected the completed A document"),
    };
    assert_eq!(
        retained_owner.target.page_key, a_key,
        "exercise the retained previous-page owner"
    );
    (app, a_key, b_key, a_draft)
}

fn assert_switched_a_saved(app: &mut App, key: &str, draft: &LocalAdjustmentLayers) {
    app.finish_local_adjust_writes();
    assert_eq!(
        app.local_adjust_db
            .as_ref()
            .unwrap()
            .get_layers(key)
            .as_deref(),
        Some(draft.as_slice())
    );
}

#[test]
fn page_edit_920_spread_switch_adoption_keeps_b_when_previous_owner_a_disappears() {
    let ctx = egui::Context::default();
    let (mut app, a_key, b_key, draft) = switched_spread_fixture(&ctx);
    let items = vec![app.items[0].clone(), app.items[2].clone()];
    app.replace_search_view_items(items, vec![None; 2]);
    assert!(
        app.local_adjust_mode,
        "B remains the same displayed edit target"
    );
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.page_path_key(0).as_deref(), Some(b_key.as_str()));
    assert!(
        app.local_adjust_spread_ctx.is_some(),
        "continuation must retain its typed pivot"
    );
    assert_switched_a_saved(&mut app, &a_key, &draft);
    assert!(
        app.local_adjust_db
            .as_ref()
            .unwrap()
            .get_layers(&b_key)
            .is_none()
    );
}

#[test]
fn page_edit_920_spread_switch_adoption_ends_b_when_only_previous_owner_a_remains() {
    let ctx = egui::Context::default();
    let (mut app, a_key, b_key, draft) = switched_spread_fixture(&ctx);
    let items = vec![app.items[1].clone(), app.items[2].clone()];
    app.replace_search_view_items(items, vec![None; 2]);
    assert!(
        !app.local_adjust_mode,
        "retaining A cannot keep the removed B editor alive"
    );
    assert!(app.local_adjust_spread_ctx.is_none());
    assert_switched_a_saved(&mut app, &a_key, &draft);
    assert!(
        app.local_adjust_db
            .as_ref()
            .unwrap()
            .get_layers(&b_key)
            .is_none()
    );
}

#[test]
fn page_edit_920_spread_switch_removal_keeps_b_when_previous_owner_a_is_deleted() {
    let ctx = egui::Context::default();
    let (mut app, a_key, b_key, draft) = switched_spread_fixture(&ctx);
    app.remove_items_batch(&[1]);
    assert!(
        app.local_adjust_mode,
        "deleting A must not end the displayed B editor"
    );
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.page_path_key(0).as_deref(), Some(b_key.as_str()));
    assert!(app.local_adjust_spread_ctx.is_some());
    assert_switched_a_saved(&mut app, &a_key, &draft);
    assert!(
        app.local_adjust_db
            .as_ref()
            .unwrap()
            .get_layers(&b_key)
            .is_none()
    );
}

#[test]
fn page_edit_920_spread_switch_removal_ends_b_when_previous_owner_a_is_not_deleted() {
    let ctx = egui::Context::default();
    let (mut app, a_key, b_key, draft) = switched_spread_fixture(&ctx);
    app.remove_items_batch(&[0]);
    assert!(
        !app.local_adjust_mode,
        "deleting the actual edit target B must end the editor"
    );
    assert!(app.local_adjust_spread_ctx.is_none());
    assert_switched_a_saved(&mut app, &a_key, &draft);
    assert!(
        app.local_adjust_db
            .as_ref()
            .unwrap()
            .get_layers(&b_key)
            .is_none()
    );
}

#[test]
fn page_edit_920_reindex_preserves_local_gesture_and_occurrence_owned_spread_pivot() {
    let mut app = fixture();
    let before = begin_local(&mut app);
    app.local_adjust_spread_ctx = app.capture_local_page_edit_pivot(pivot());
    let draft = app.local_adjust_page_layers[&1].clone();
    let items = vec![
        app.items[1].clone(),
        app.items[0].clone(),
        app.items[1].clone(),
    ];
    let swap = app.prepare_rating_membership_items_swap(&items);
    app.replace_search_view_items(items, vec![None; 3]);
    app.finish_rating_membership_viewer_swap(swap);
    assert!(app.local_adjust_mode);
    assert_eq!(app.fullscreen_idx, Some(2));
    let session = app.local_adjust_shape_key_edit.as_ref().unwrap();
    assert_eq!(session.fs_idx, 2);
    assert_eq!(session.owner.slots, [0, 2]);
    assert_eq!(session.before, before);
    assert_eq!(app.local_adjust_page_layers[&0], draft);
    assert_eq!(app.local_adjust_page_layers[&2], draft);
    let pivot = app.local_adjust_spread_ctx.as_ref().unwrap();
    assert_eq!(pivot.pair(), Some((1, 0)));
    assert_eq!(pivot.anchor(), Some(1));
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(2, 0.125, 0.0));
    assert_eq!(app.meta_undo.undo_len(), 0);
    app.local_adjust_mode = false;
    app.fold_local_adjust_edit_state();
    app.reconcile_page_edit_spread_pivots();
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.spread_mode, SpreadMode::Ltr);
    app.finish_local_adjust_writes();
}

#[test]
fn page_edit_920_same_identity_replacement_exits_each_index_owned_tool() {
    for tool in 0..5 {
        let mut app = fixture();
        match tool {
            0 => {
                app.conceal_mode = true;
                app.conceal_spread_ctx = Some(pivot());
            }
            1 => {
                app.erase_mode = true;
                app.erase_spread_ctx = Some(pivot());
            }
            2 => {
                app.text_mode = true;
                app.text_spread_ctx = Some(pivot());
            }
            3 => {
                app.export_crop_mode = true;
                app.export_crop_spread_ctx = Some(pivot());
            }
            _ => {
                app.sns_split = Some(crate::sns_split::SnsSplitLayout::centered_max(
                    crate::sns_split::SnsTarget::X,
                    2,
                    [4, 3],
                ));
                app.sns_split_spread_ctx = Some(pivot());
            }
        }
        let items = vec![app.items[1].clone(), app.items[0].clone()];
        app.replace_search_view_items(items, vec![None; 2]);
        assert!(!app.is_overlay_edit_mode_active(), "tool={tool}");
        assert!(app.conceal_spread_ctx.is_none());
        assert!(app.erase_spread_ctx.is_none());
        assert!(app.text_spread_ctx.is_none());
        assert!(app.export_crop_spread_ctx.is_none());
        assert!(app.sns_split_spread_ctx.is_none());
    }
}

#[test]
fn page_edit_920_replacement_without_local_target_finishes_old_document_before_install() {
    let mut app = fixture();
    begin_local(&mut app);
    let key = app.page_path_key(1).unwrap();
    let draft = app.local_adjust_page_layers[&1].clone();
    app.local_adjust_spread_ctx = app.capture_local_page_edit_pivot(pivot());
    let items = vec![app.items[0].clone(), app.items[2].clone()];
    app.replace_search_view_items(items, vec![None; 2]);
    assert!(!app.local_adjust_mode);
    assert!(app.local_adjust_shape_key_edit.is_none());
    assert!(app.local_adjust_spread_ctx.is_none());
    assert_eq!(app.meta_undo.undo_len(), 1);
    app.finish_local_adjust_writes();
    assert_eq!(
        app.local_adjust_db
            .as_ref()
            .unwrap()
            .get_layers(&key)
            .as_deref(),
        Some(draft.as_slice())
    );
    let landing = app.fullscreen_idx;
    app.reconcile_page_edit_spread_pivots();
    assert_eq!(app.fullscreen_idx, landing);
}

#[test]
fn page_edit_920_removing_prefix_reprojects_local_gesture_without_finishing_it() {
    let mut app = fixture();
    let before = begin_local(&mut app);
    app.remove_items_batch(&[0]);
    assert!(app.local_adjust_mode);
    assert_eq!(app.fullscreen_idx, Some(0));
    let session = app.local_adjust_shape_key_edit.as_ref().unwrap();
    assert_eq!(session.fs_idx, 0);
    assert_eq!(session.owner.slots, [0]);
    assert_eq!(session.before, before);
    assert_eq!(app.meta_undo.undo_len(), 0);
    app.commit_local_adjust_pending_edits();
    app.finish_local_adjust_writes();
}

#[test]
fn page_edit_920_removing_edited_occurrence_finishes_even_if_same_key_alias_remains() {
    let mut app = fixture();
    app.items[2] = app.items[1].clone();
    begin_local(&mut app);
    app.remove_items_batch(&[1]);
    assert!(!app.local_adjust_mode);
    assert!(app.local_adjust_shape_key_edit.is_none());
    assert_eq!(app.meta_undo.undo_len(), 1);
    app.finish_local_adjust_writes();
}

#[test]
fn page_edit_920_snapshot_subset_retains_local_owner_through_activate_and_restore() {
    let mut app = fixture();
    let before = begin_local(&mut app);
    app.visible_indices = vec![1, 2];
    app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    assert!(app.local_adjust_mode);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.local_adjust_shape_key_edit.as_ref().unwrap().fs_idx, 0);
    assert_eq!(
        app.local_adjust_shape_key_edit.as_ref().unwrap().before,
        before
    );
    app.deactivate_snapshot();
    assert!(app.local_adjust_mode);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.local_adjust_shape_key_edit.as_ref().unwrap().fs_idx, 1);
    assert_eq!(
        app.local_adjust_shape_key_edit.as_ref().unwrap().before,
        before
    );
    assert_eq!(app.meta_undo.undo_len(), 0);
    app.commit_local_adjust_pending_edits();
    app.finish_local_adjust_writes();
}
