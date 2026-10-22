//! Regression coverage for context-owned drafts, shared history and writer delivery.
#![cfg(test)]

use super::tests::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use crate::undo_stack::UndoEntry;
use crossbeam_channel::{Receiver, Sender};
use local_adjust_core::{LocalAdjustmentLayer, LocalAdjustmentLayers, LocalEffect, LocalMask};

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
        LocalEffect::None,
    )])
}

fn install(app: &mut App, paths: &[PathBuf], layers: &[LocalAdjustmentLayers]) {
    app.items = paths.iter().cloned().map(GridItem::Image).collect();
    app.thumbnails = vec![ThumbnailState::Pending; paths.len()];
    app.image_metas = vec![None; paths.len()];
    app.visible_indices = (0..paths.len()).collect();
    app.fullscreen_idx = Some(0);
    app.selected = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.local_adjust_mode = true;
    app.local_adjust_selected_shape = Some(0);
    app.local_adjust_mask_edit_target = LocalAdjustMaskEditTarget::Base;
    app.local_adjust_page_layers.clear();
    app.local_adjust_pages.clear();
    for (idx, layers) in layers.iter().enumerate() {
        app.set_local_adjust_layers_for_idx_memory_only(idx, layers.clone());
    }
    app.page_edit_snapshot = Some(page_edit_snapshot::PageEditSnapshot {
        order: Arc::new(page_edit_snapshot::PageKeyOrder::from_items(&app.items).unwrap()),
        ..Default::default()
    });
}

fn pages(names: &[&str]) -> (AppTestEnv, Vec<PathBuf>) {
    let mut app = setup_app();
    app.settings.sidecar_backup_enabled = true;
    let paths = names
        .iter()
        .map(|name| {
            let folder = app.tmp.path().join(name);
            std::fs::create_dir_all(&folder).unwrap();
            let path = folder.join("page.png");
            image::RgbaImage::from_pixel(1, 1, image::Rgba([90, 100, 110, 255]))
                .save(&path)
                .unwrap();
            path
        })
        .collect::<Vec<_>>();
    let layers = names.iter().map(|name| document(name)).collect::<Vec<_>>();
    install(&mut app, &paths, &layers);
    for idx in 0..paths.len() {
        let target = app.page_adjustment_target_for_idx(idx).unwrap();
        app.set_local_adjust_layers_for_target(&target, layers[idx].clone());
    }
    app.finish_local_adjust_writes();
    (app, paths)
}

fn assert_saved(app: &App, target: &PageAdjustmentTarget, expected: &LocalAdjustmentLayers) {
    assert_eq!(
        app.local_adjust_db
            .as_ref()
            .unwrap()
            .get_layers(&target.page_key)
            .as_deref(),
        Some(expected.as_slice()),
        "durable page contents"
    );
    let (folder, item_key) = target.sidecar_coords.as_ref().unwrap();
    assert_eq!(
        app.sidecars
            .get(folder)
            .unwrap()
            .items()
            .get(item_key)
            .and_then(|item| item.local_adjust_layers.as_ref()),
        Some(expected),
        "sidecar mirror contents"
    );
}

/// A failed assertion must release the worker before AppTestEnv joins/drops it.
struct WritePause {
    reached: Receiver<()>,
    resume: Option<Sender<()>>,
}

impl WritePause {
    fn next(app: &mut App) -> Self {
        app.ensure_local_adjust_write_handle();
        let (reached, resume) = app
            .local_adjust_write_handle
            .as_ref()
            .unwrap()
            .pause_next_write_for_test();
        Self {
            reached,
            resume: Some(resume),
        }
    }

    fn wait(&self) {
        self.reached
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("writer must reach the requested stop before the assertion");
    }

    fn release(&mut self) {
        if let Some(resume) = self.resume.take() {
            let _ = resume.send(());
        }
    }
}

impl Drop for WritePause {
    fn drop(&mut self) {
        self.release();
    }
}

#[test]
fn key_run_undo_before_idle_is_not_recommitted_by_timer_or_redo() {
    let (mut app, _) = pages(&["a", "b"]);
    let target_a = app.page_adjustment_target_for_idx(0).unwrap();
    let target_b = app.page_adjustment_target_for_idx(1).unwrap();
    let before = app.local_adjust_page_layers[&0].clone();
    let sibling = app.local_adjust_page_layers[&1].clone();

    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));
    let after = app.local_adjust_page_layers[&0].clone();
    assert!(app.local_adjust_shape_key_edit.is_some());
    assert_eq!(app.meta_undo.undo_len(), 0);
    app.apply_meta_undo();
    app.finish_local_adjust_writes();

    assert!(app.local_adjust_shape_key_edit.is_none());
    assert_saved(&app, &target_a, &before);
    assert_saved(&app, &target_b, &sibling);
    assert_eq!(app.meta_undo.redo_len(), 1);
    assert!(!app.settle_local_adjust_shape_key_edit());
    app.fold_local_adjust_edit_state();
    app.poll_local_adjust_write_results();
    assert_eq!(app.meta_undo.redo_len(), 1);
    assert_eq!(app.local_adjust_page_layers[&0], before);
    assert_eq!(app.local_adjust_page_layers[&1], sibling);

    app.apply_meta_redo();
    app.finish_local_adjust_writes();
    assert_saved(&app, &target_a, &after);
    assert_saved(&app, &target_b, &sibling);
    assert_eq!(app.meta_undo.undo_len(), 1);
    assert_eq!(app.meta_undo.redo_len(), 0);
}

#[cfg(windows)]
#[test]
fn same_key_new_context_uses_accepted_document_before_writer_completion() {
    let (mut app, paths) = pages(&["shared"]);
    let target = app.page_adjustment_target_for_idx(0).unwrap();
    let s0 = app.local_adjust_page_layers[&0].clone();
    let sibling = app.build_window_context_for_test(137201, |owner| {
        install(owner, &paths, std::slice::from_ref(&s0));
    });

    let mut first_pause = WritePause::next(&mut app);
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));
    let s1 = app.local_adjust_page_layers[&0].clone();
    app.persist_local_adjust_shape_key_edit();
    first_pause.wait();
    assert_saved(&app, &target, &s0);

    let after = app
        .with_viewer_context(sibling, |owner| {
            assert!(owner.begin_local_page_edit(0));
            assert_eq!(owner.local_adjust_page_layers[&0], s1);
            assert!(owner.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));
            assert_eq!(
                owner.local_adjust_shape_key_edit.as_ref().unwrap().before,
                s1
            );
            let after = owner.local_adjust_page_layers[&0].clone();
            owner.apply_meta_undo();
            assert_eq!(owner.local_adjust_page_layers[&0], s1);
            owner.apply_meta_redo();
            assert_eq!(owner.local_adjust_page_layers[&0], after);
            after
        })
        .unwrap();

    // Pause the following B write too, so A's durable S1 and mirror can be
    // checked separately from the eventual latest B document.
    let mut second_pause = WritePause::next(&mut app);
    first_pause.release();
    second_pause.wait();
    app.poll_local_adjust_write_results();
    assert_saved(&app, &target, &s1);
    second_pause.release();
    app.finish_local_adjust_writes();
    assert_saved(&app, &target, &after);
    app.with_viewer_context(sibling, |owner| {
        assert_eq!(owner.local_adjust_page_layers[&0], after);
        assert!(owner.local_adjust_shape_key_edit.is_none());
    })
    .unwrap();
}

#[cfg(windows)]
#[test]
fn parked_key_run_is_registered_before_actual_rating_handler() {
    let (mut app, paths) = pages(&["a", "b"]);
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));
    let parked = app.stash_mounted_and_start_fresh("page_edit_history_test");
    install(&mut app, &paths[1..], &[document("b")]);
    assert!(app.local_adjust_shape_key_edit.is_none());
    assert_eq!(app.meta_undo.undo_len(), 0);

    assert!(app.set_rating(0, 4));
    assert_eq!(app.meta_undo.undo_len(), 2);
    assert!(matches!(
        app.meta_undo.peek_undo(),
        Some(UndoEntry::Rating { .. })
    ));
    app.with_viewer_context(parked, |owner| {
        assert!(owner.local_adjust_shape_key_edit.is_none());
        assert!(!owner.settle_local_adjust_shape_key_edit());
    })
    .unwrap();
    assert_eq!(app.meta_undo.undo_len(), 2);
    let UndoEntry::Rating { changes, .. } = app.meta_undo.pop_undo().unwrap() else {
        panic!("the later rating must be the newest history operation");
    };
    assert_eq!(changes[0].source_path, paths[1]);
    let UndoEntry::LocalAdjustment { changes, .. } = app.meta_undo.pop_undo().unwrap() else {
        panic!("the earlier parked edit must precede the rating");
    };
    assert_eq!(
        changes[0].target.page_key,
        crate::adjustment_db::normalize_path(&paths[0])
    );
    app.finish_local_adjust_writes();
}

#[cfg(windows)]
#[test]
fn stopping_flushes_parked_key_run_and_cancels_mounted_unfinished_lasso() {
    let (mut app, paths) = pages(&["a", "b"]);
    let target_a = app.page_adjustment_target_for_idx(0).unwrap();
    let target_b = app.page_adjustment_target_for_idx(1).unwrap();
    let before_b = app.local_adjust_page_layers[&1].clone();
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));
    let after_a = app.local_adjust_page_layers[&0].clone();
    let parked = app.stash_mounted_and_start_fresh("page_edit_shutdown_test");
    install(&mut app, &paths[1..], std::slice::from_ref(&before_b));
    app.local_adjust_mask_tool = LocalAdjustMaskTool::Lasso;
    app.local_adjust_mask_lasso_points = vec![[0.0, 0.0], [0.5, 0.5]];
    app.with_viewer_context(parked, |owner| {
        assert!(owner.local_adjust_shape_key_edit.is_some());
    })
    .unwrap();
    assert_eq!(
        app.meta_undo.undo_len(),
        0,
        "park itself does not commit the key run"
    );

    app.persist_window_state_and_flush(PersistScope::ProcessIsStopping);

    assert!(app.local_adjust_write_handle.is_none());
    assert!(app.local_adjust_write_pending.is_empty());
    assert!(app.local_adjust_mask_lasso_points.is_empty());
    assert_saved(&app, &target_a, &after_a);
    assert_saved(&app, &target_b, &before_b);
    assert_eq!(
        app.meta_undo.undo_len(),
        1,
        "unfinished lasso adds no history"
    );
    let (folder, item_key) = target_a.sidecar_coords.as_ref().unwrap();
    assert!(folder.join(crate::sidecar::SIDECAR_FILENAME).is_file());
    let disk_sidecar = crate::sidecar::SidecarFile::load(folder);
    assert_eq!(
        disk_sidecar.items()[item_key].local_adjust_layers.as_ref(),
        Some(&after_a)
    );
    app.with_viewer_context(parked, |owner| {
        assert!(owner.local_adjust_shape_key_edit.is_none());
        assert!(!owner.settle_local_adjust_shape_key_edit());
    })
    .unwrap();
    assert!(
        app.local_adjust_write_handle.is_none(),
        "remount must not enqueue twice"
    );
}

#[test]
fn canonical_mutation_drops_all_index_aliases_before_copy_on_write() {
    let mut app = setup_app();
    let path = app.tmp.path().join("duplicate.png");
    let layers = document("original");
    install(&mut app, &[path.clone(), path], &[layers.clone(), layers]);
    assert!(app.begin_local_page_edit(0));
    let original = Arc::as_ptr(&app.local_adjust_page_layers[&0]);
    assert!(Arc::ptr_eq(
        &app.local_adjust_page_layers[&0],
        &app.local_adjust_page_layers[&1]
    ));

    for name in ["first input frame", "second input frame"] {
        let draft = app.local_page_edit_layers_mut(0).unwrap();
        assert_eq!(
            Arc::strong_count(draft),
            1,
            "rendering must not retain a COW alias"
        );
        assert_eq!(Arc::as_ptr(draft), original);
        Arc::make_mut(draft)[0].name = name.to_owned();
        assert_eq!(
            Arc::as_ptr(draft),
            original,
            "each frame must reuse the canonical allocation"
        );
        app.project_local_page_edit_document(0);
        assert_eq!(app.local_adjust_page_layers[&0][0].name, name);
        assert_eq!(app.local_adjust_page_layers[&1][0].name, name);
        assert!(Arc::ptr_eq(
            &app.local_adjust_page_layers[&0],
            &app.local_adjust_page_layers[&1]
        ));
    }
}

#[cfg(windows)]
#[test]
fn foreign_completion_releases_only_its_origin_while_sibling_draft_stays_volatile() {
    let (mut app, paths) = pages(&["a", "b"]);
    let target_a = app.page_adjustment_target_for_idx(0).unwrap();
    let target_b = app.page_adjustment_target_for_idx(1).unwrap();
    let mut pause = WritePause::next(&mut app);
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));
    let after_a = app.local_adjust_page_layers[&0].clone();
    app.persist_local_adjust_shape_key_edit();
    pause.wait();
    let parked = app.stash_mounted_and_start_fresh("page_edit_completion_test");
    install(&mut app, &paths[1..], &[document("b")]);
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));
    let draft_b = app.local_adjust_page_layers[&0].clone();
    assert!(
        app.page_edit_snapshot
            .as_ref()
            .unwrap()
            .volatile_keys
            .contains(&target_b.page_key)
    );

    pause.release();
    app.finish_local_adjust_writes();
    assert_saved(&app, &target_a, &after_a);
    assert!(
        app.page_edit_snapshot
            .as_ref()
            .unwrap()
            .volatile_keys
            .contains(&target_b.page_key)
    );
    assert_eq!(app.local_adjust_page_layers[&0], draft_b);
    assert!(app.local_adjust_shape_key_edit.is_some());
    app.with_viewer_context(parked, |owner| {
        assert!(
            !owner
                .page_edit_snapshot
                .as_ref()
                .unwrap()
                .volatile_keys
                .contains(&target_a.page_key)
        );
        assert_eq!(owner.local_adjust_page_layers[&0], after_a);
    })
    .unwrap();
    assert_eq!(app.local_adjust_page_layers[&0], draft_b);
    assert!(app.local_adjust_shape_key_edit.is_some());
    app.commit_local_adjust_pending_edits();
    app.finish_local_adjust_writes();
}

#[test]
fn selected_third_layer_survives_repeated_stream_adoption_and_key_continuation() {
    let (mut app, paths) = pages(&["a", "b"]);
    let target_a = app.page_adjustment_target_for_idx(0).unwrap();
    let target_b = app.page_adjustment_target_for_idx(1).unwrap();
    let sibling = app.local_adjust_page_layers[&1].clone();
    let before = LocalAdjustmentLayers::new(
        ["first", "second", "third"]
            .into_iter()
            .map(|name| document(name)[0].clone())
            .collect(),
    );
    app.set_local_adjust_layers_for_target(&target_a, before.clone());
    app.finish_local_adjust_writes();
    app.select_local_page_edit_layer(0, 2);
    assert!(matches!(
        &app.local_page_edit_document,
        LocalPageEditDocument::Selection {
            selected_layer: 2,
            ..
        }
    ));
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));

    for order in [vec![1, 0], vec![0, 1, 0], vec![1, 0]] {
        let items = order
            .iter()
            .map(|idx| GridItem::Image(paths[*idx].clone()))
            .collect::<Vec<_>>();
        let a_slots = order
            .iter()
            .enumerate()
            .filter_map(|(slot, idx)| (*idx == 0).then_some(slot))
            .collect::<Vec<_>>();
        app.replace_search_view_items(items, vec![None; order.len()]);
        let a_idx = a_slots[0];
        assert_eq!(app.selected_local_adjust_layer_idx(a_idx), Some(2));
        assert!(matches!(
            &app.local_page_edit_document,
            LocalPageEditDocument::Editing {
                selected_layer: 2,
                ..
            }
        ));
        let session = app.local_adjust_shape_key_edit.as_ref().unwrap();
        assert_eq!(session.layer_idx, 2);
        assert_eq!(session.owner.target.page_key, target_a.page_key);
        assert_eq!(session.owner.slots, a_slots);
        assert_eq!(session.before, before);
        assert!(app.nudge_selected_local_adjust_shape_from_shortcut(a_idx, 0.125, 0.0));
        let current = &app.local_adjust_page_layers[&a_idx];
        assert_eq!(current[0], before[0]);
        assert_eq!(current[1], before[1]);
        assert_ne!(current[2], before[2]);
        for slot in a_slots {
            assert_eq!(app.local_adjust_page_layers[&slot], *current);
            assert_eq!(app.local_adjust_selected_layers[&slot], 2);
        }
        assert_eq!(
            app.meta_undo.undo_len(),
            0,
            "adoption must not split the run"
        );
    }

    let a_idx = app.local_adjust_shape_key_edit.as_ref().unwrap().fs_idx;
    let after = app.local_adjust_page_layers[&a_idx].clone();
    app.commit_local_adjust_pending_edits();
    app.finish_local_adjust_writes();
    assert_eq!(app.meta_undo.undo_len(), 1);
    assert_saved(&app, &target_a, &after);
    assert_saved(&app, &target_b, &sibling);
}

fn unfinished_gesture(app: &mut App, tool: LocalAdjustMaskTool) {
    app.local_adjust_mask_tool = tool;
    match tool {
        LocalAdjustMaskTool::Lasso => {
            app.local_adjust_mask_lasso_points = vec![[0.125, 0.25], [0.5, 0.75]];
        }
        LocalAdjustMaskTool::Rect => {
            app.local_adjust_mask_shape_drag_start = Some([0.125, 0.25]);
            app.local_adjust_mask_shape_drag_end = Some([0.5, 0.75]);
        }
        _ => unreachable!("the fixture models an unfinished lasso or new shape"),
    }
}

fn assert_unfinished(app: &App, tool: LocalAdjustMaskTool) {
    match tool {
        LocalAdjustMaskTool::Lasso => {
            assert_eq!(
                app.local_adjust_mask_lasso_points,
                [[0.125, 0.25], [0.5, 0.75]]
            );
        }
        LocalAdjustMaskTool::Rect => {
            assert_eq!(app.local_adjust_mask_shape_drag_start, Some([0.125, 0.25]));
            assert_eq!(app.local_adjust_mask_shape_drag_end, Some([0.5, 0.75]));
        }
        _ => unreachable!(),
    }
}

#[cfg(windows)]
#[test]
fn shared_history_preserves_parked_unfinished_gesture_and_undo_cancels_only_receiver() {
    for tool in [LocalAdjustMaskTool::Lasso, LocalAdjustMaskTool::Rect] {
        let (mut app, paths) = pages(&["a", "b"]);
        let target_a = app.page_adjustment_target_for_idx(0).unwrap();
        let target_b = app.page_adjustment_target_for_idx(1).unwrap();
        let before_a = app.local_adjust_page_layers[&0].clone();
        let before_b = app.local_adjust_page_layers[&1].clone();
        assert!(app.begin_local_page_edit(0));
        unfinished_gesture(&mut app, tool);
        let parked = app.stash_mounted_and_start_fresh("page_edit_unfinished_history_test");
        install(&mut app, &paths[1..], std::slice::from_ref(&before_b));

        assert!(app.set_rating(0, 4));
        assert_eq!(app.meta_undo.undo_len(), 1);
        app.with_viewer_context(parked, |owner| {
            assert_unfinished(owner, tool);
            assert_eq!(owner.local_adjust_page_layers[&0], before_a);
        })
        .unwrap();

        assert!(app.begin_local_page_edit(0));
        unfinished_gesture(&mut app, tool);
        app.apply_meta_undo();
        assert!(app.local_adjust_mask_lasso_points.is_empty());
        assert!(app.local_adjust_mask_shape_drag_start.is_none());
        assert!(app.local_adjust_mask_shape_drag_end.is_none());
        assert_eq!(app.meta_undo.undo_len(), 0);
        assert_eq!(app.meta_undo.redo_len(), 1);
        app.with_viewer_context(parked, |owner| {
            assert_unfinished(owner, tool);
            assert_eq!(owner.local_adjust_page_layers[&0], before_a);
        })
        .unwrap();
        assert_saved(&app, &target_a, &before_a);
        assert_saved(&app, &target_b, &before_b);
        assert!(
            app.local_adjust_write_handle.is_none(),
            "unfinished input must not save"
        );
    }
}

#[cfg(windows)]
#[test]
fn stale_panel_parameter_edit_preserves_latest_accepted_shape_in_other_context() {
    let (mut app, paths) = pages(&["shared"]);
    let target = app.page_adjustment_target_for_idx(0).unwrap();
    let s0 = app.local_adjust_page_layers[&0].clone();
    let sibling = app.build_window_context_for_test(137204, |owner| {
        install(owner, &paths, std::slice::from_ref(&s0));
    });
    // B's UI pass read S0 before A's key edit. Only opacity was operated;
    // the old mask in this intent must never replace A's accepted shape.
    let old_layer = s0[0].clone();
    let mut edited_layer = old_layer.clone();
    edited_layer.opacity = 0.5;
    let edit = crate::local_adjust_parameter_edit::LocalLayerParameterEdit {
        before: old_layer,
        after: edited_layer,
    };
    let mut pause = WritePause::next(&mut app);
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));
    let s1 = app.local_adjust_page_layers[&0].clone();
    app.persist_local_adjust_shape_key_edit();
    pause.wait();
    assert_saved(&app, &target, &s0);
    let mut expected = s1.clone();
    Arc::make_mut(&mut expected)[0].opacity = 0.5;

    app.with_viewer_context(sibling, |owner| {
        owner.apply_local_adjust_parameter_edit_for_test(0, 0, edit);
        assert_eq!(owner.local_adjust_page_layers[&0], expected);
        assert_eq!(owner.local_adjust_page_layers[&0][0].mask, s1[0].mask);
        let UndoEntry::LocalAdjustment { changes, .. } = owner.meta_undo.peek_undo().unwrap()
        else {
            panic!("the real panel application must capture local history");
        };
        assert_eq!(changes[0].before, s1);
        assert_eq!(changes[0].after, expected);
        owner.apply_meta_undo();
        assert_eq!(owner.local_adjust_page_layers[&0], s1);
        owner.apply_meta_redo();
        assert_eq!(owner.local_adjust_page_layers[&0], expected);
    })
    .unwrap();
    pause.release();
    app.finish_local_adjust_writes();
    assert_saved(&app, &target, &expected);
    assert_eq!(app.meta_undo.undo_len(), 2);
}

#[test]
fn actual_slider_frame_start_updates_and_release_capture_exactly_one_undo() {
    for scope in [AdjustScopeSelection::Page, AdjustScopeSelection::Standard] {
        let (mut app, _) = pages(&["a", "b"]);
        app.local_adjust_mode = false;
        let target = app.page_adjustment_target_for_idx(0).unwrap();
        let mut before = crate::adjustment::AdjustParams::default();
        before.brightness = 5.0;
        match scope {
            AdjustScopeSelection::Page => app.set_page_params(0, before.clone()),
            AdjustScopeSelection::Standard => app.settings.global_preset = before.clone(),
        }
        let sibling_before = app.effective_params(1).clone();
        let mut latest = before.clone();
        for brightness in [10.0, 20.0, 30.0] {
            let original = latest.clone();
            latest.brightness = brightness;
            app.apply_adjustment_panel_edit_frame(
                0,
                scope,
                original,
                latest.clone(),
                true,
                true,
                false,
            );
            assert!(app.adjustment_dragging);
            assert_eq!(
                app.meta_undo.undo_len(),
                0,
                "input frames must not split a drag"
            );
            match scope {
                AdjustScopeSelection::Page => {
                    let session = app.adjustment_drag_session.as_ref().unwrap();
                    assert_eq!(session.before, Some(before.clone()));
                    assert_eq!(session.draft, Some(latest.clone()));
                    assert_eq!(app.adjustment_page_params[&0], latest);
                    assert_eq!(
                        app.adjustment_db
                            .as_ref()
                            .unwrap()
                            .get_page_params(&target.page_key),
                        Some(before.clone()),
                        "slider previews are not durable until release"
                    );
                    assert_eq!(*app.effective_params(1), sibling_before);
                }
                AdjustScopeSelection::Standard => {
                    let session = app.adjustment_standard_drag_session.as_ref().unwrap();
                    assert_eq!(session.before, before);
                    assert_eq!(session.draft, latest);
                    assert_eq!(app.settings.global_preset, latest);
                }
            }
        }

        for _ in 0..2 {
            app.apply_adjustment_panel_edit_frame(
                0,
                scope,
                latest.clone(),
                latest.clone(),
                false,
                false,
                false,
            );
            assert!(!app.adjustment_dragging);
            assert!(app.adjustment_drag_session.is_none());
            assert!(app.adjustment_standard_drag_session.is_none());
            assert_eq!(app.meta_undo.undo_len(), 1, "release is delivered once");
        }
        let UndoEntry::Adjustment { changes, .. } = app.meta_undo.peek_undo().unwrap() else {
            panic!("the real slider frame handler must capture adjustment history");
        };
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].before, Some(before.clone()));
        assert_eq!(changes[0].after, Some(latest.clone()));
        app.apply_meta_undo();
        match scope {
            AdjustScopeSelection::Page => assert_eq!(app.adjustment_page_params[&0], before),
            AdjustScopeSelection::Standard => assert_eq!(app.settings.global_preset, before),
        }
        app.apply_meta_redo();
        match scope {
            AdjustScopeSelection::Page => {
                assert_eq!(app.adjustment_page_params[&0], latest);
                assert_eq!(
                    app.adjustment_db
                        .as_ref()
                        .unwrap()
                        .get_page_params(&target.page_key),
                    Some(latest.clone())
                );
                assert_eq!(*app.effective_params(1), sibling_before);
            }
            AdjustScopeSelection::Standard => assert_eq!(app.settings.global_preset, latest),
        }
        assert_eq!(app.meta_undo.undo_len(), 1);
        assert_eq!(app.meta_undo.redo_len(), 0);
    }
}

#[cfg(windows)]
#[test]
fn local_spread_pivot_stays_with_context_and_restores_exact_duplicate_anchor() {
    let (mut app, paths) = pages(&["a", "b", "c"]);
    let a_paths = vec![paths[0].clone(), paths[0].clone(), paths[1].clone()];
    install(
        &mut app,
        &a_paths,
        &[document("a"), document("a"), document("b")],
    );
    app.local_adjust_spread_ctx = app.capture_local_page_edit_pivot(PageEditSpreadPivot {
        saved_mode: crate::settings::SpreadMode::Rtl,
        pair: (1, 2),
        navigation_anchor_idx: 1,
    });
    app.enter_page_edit_single_view(1);
    app.fs_zoom = 1.75;
    app.fs_pan = egui::vec2(12.0, 24.0);
    let parked = app.stash_mounted_and_start_fresh("local_spread_pivot_owner_test");

    install(
        &mut app,
        &[paths[2].clone(), paths[1].clone()],
        &[document("c"), document("b")],
    );
    app.local_adjust_spread_ctx = app.capture_local_page_edit_pivot(PageEditSpreadPivot {
        saved_mode: crate::settings::SpreadMode::Ltr,
        pair: (0, 1),
        navigation_anchor_idx: 1,
    });
    app.enter_page_edit_single_view(0);
    app.fs_zoom = 2.25;
    app.fs_pan = egui::vec2(-12.0, -24.0);

    app.with_viewer_context(parked, |owner| {
        assert!(owner.local_adjust_mode);
        assert_eq!(owner.spread_mode, crate::settings::SpreadMode::Single);
        assert_eq!(owner.fullscreen_idx, Some(1));
        assert_eq!(owner.fs_zoom, 1.75);
        assert_eq!(owner.fs_pan, egui::vec2(12.0, 24.0));
        assert_eq!(owner.page_edit_spread_pair(), Some((1, 2)));
        assert_eq!(
            owner.local_adjust_spread_ctx.as_ref().unwrap().anchor(),
            Some(1)
        );

        for (order, pair, anchor) in [
            (vec![1, 0, 2, 0], (3, 0), 3),
            (vec![0, 2, 0, 1], (2, 3), 2),
            (vec![2, 0, 1, 0], (3, 2), 3),
        ] {
            let items = order
                .iter()
                .map(|idx| GridItem::Image(paths[*idx].clone()))
                .collect();
            owner.replace_search_view_items(items, vec![None; order.len()]);
            assert!(owner.local_adjust_mode, "adoption is not an editor exit");
            assert_eq!(owner.page_edit_spread_pair(), Some(pair));
            assert_eq!(
                owner.local_adjust_spread_ctx.as_ref().unwrap().anchor(),
                Some(anchor)
            );
            assert_eq!(owner.items[anchor], GridItem::Image(paths[0].clone()));
            assert_eq!(
                owner.items[..anchor]
                    .iter()
                    .filter(|item| **item == GridItem::Image(paths[0].clone()))
                    .count(),
                1,
                "the saved anchor was the second occurrence, not the first match"
            );
        }
        owner.local_adjust_mode = false;
        owner.reconcile_page_edit_spread_pivots();
        assert_eq!(owner.spread_mode, crate::settings::SpreadMode::Rtl);
        assert_eq!(owner.fullscreen_idx, Some(3));
        assert!(owner.local_adjust_spread_ctx.is_none());
        assert_eq!(owner.fs_zoom, 1.0);
        assert_eq!(owner.fs_pan, egui::Vec2::ZERO);
    })
    .unwrap();

    assert!(app.local_adjust_mode);
    assert_eq!(app.spread_mode, crate::settings::SpreadMode::Single);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.fs_zoom, 2.25);
    assert_eq!(app.fs_pan, egui::vec2(-12.0, -24.0));
    assert_eq!(app.page_edit_spread_pair(), Some((0, 1)));
    assert_eq!(
        app.local_adjust_spread_ctx.as_ref().unwrap().anchor(),
        Some(1)
    );
}

#[cfg(windows)]
#[test]
fn parked_page_drag_normalized_noop_removes_only_its_preview_and_preserves_redo() {
    let (mut app, paths) = pages(&["a", "b"]);
    let target_a = app.page_adjustment_target_for_idx(0).unwrap();
    let target_b = app.page_adjustment_target_for_idx(1).unwrap();
    let mut sibling_params = crate::adjustment::AdjustParams::default();
    sibling_params.brightness = 23.0;
    app.set_page_params(1, sibling_params.clone());
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(1, 0.125, 0.0));
    app.apply_meta_undo();
    app.finish_local_adjust_writes();
    assert_eq!(app.meta_undo.redo_len(), 1);
    app.local_adjust_mode = false;
    let mut latest = crate::adjustment::AdjustParams::default();
    for (brightness, changed) in [(0.0, false), (10.0, true), (0.0, true)] {
        let original = latest.clone();
        latest.brightness = brightness;
        app.apply_adjustment_panel_edit_frame(
            0,
            AdjustScopeSelection::Page,
            original,
            latest.clone(),
            changed,
            true,
            false,
        );
    }
    let session = app.adjustment_drag_session.as_ref().unwrap();
    assert_eq!(session.before, None);
    assert_eq!(
        session.draft,
        Some(crate::adjustment::AdjustParams::default())
    );
    assert!(app.adjustment_page_params.contains_key(&0));
    let parked = app.stash_mounted_and_start_fresh("parked_page_noop_test");
    install(&mut app, &paths[1..], &[document("b")]);
    app.adjustment_page_params.insert(0, sibling_params.clone());
    app.page_edit_snapshot
        .as_mut()
        .unwrap()
        .adjustment
        .insert(target_b.page_key.clone(), sibling_params.clone());

    assert!(app.begin_local_page_edit(0));
    assert_eq!(app.meta_undo.redo_len(), 1);
    assert_eq!(app.meta_undo.undo_len(), 0);
    assert_eq!(app.adjustment_page_params[&0], sibling_params);
    assert!(!app.adjusted_page_keys.contains(&target_a.page_key));
    assert!(app.adjusted_page_keys.contains(&target_b.page_key));
    assert_eq!(
        app.adjustment_db
            .as_ref()
            .unwrap()
            .get_page_params(&target_a.page_key),
        None
    );
    assert_eq!(
        app.adjustment_db
            .as_ref()
            .unwrap()
            .get_page_params(&target_b.page_key),
        Some(sibling_params.clone())
    );
    app.with_viewer_context(parked, |owner| {
        assert!(owner.adjustment_drag_session.is_none());
        assert!(
            !owner.adjustment_page_params.contains_key(&0),
            "normalized no-op has no page override badge"
        );
        assert!(
            !owner
                .page_edit_snapshot
                .as_ref()
                .unwrap()
                .adjustment
                .contains_key(&target_a.page_key)
        );
    })
    .unwrap();
    assert_eq!(app.adjustment_page_params[&0], sibling_params);
    assert_eq!(app.meta_undo.redo_len(), 1);
    assert!(app.local_adjust_write_handle.is_none());
}

#[cfg(windows)]
#[test]
fn resuming_unfinished_same_key_gesture_starts_keys_from_sibling_latest_document() {
    for completion_first in [false, true] {
        for tool in [LocalAdjustMaskTool::Lasso, LocalAdjustMaskTool::Rect] {
            let (mut app, paths) = pages(&["shared"]);
            let target = app.page_adjustment_target_for_idx(0).unwrap();
            let s0 = app.local_adjust_page_layers[&0].clone();
            assert!(app.begin_local_page_edit(0));
            unfinished_gesture(&mut app, tool);
            let parked = app.stash_mounted_and_start_fresh("unfinished_same_key_latest_test");
            install(&mut app, &paths, std::slice::from_ref(&s0));
            assert!(app.nudge_selected_local_adjust_shape_from_shortcut(0, 0.0, 0.125));
            let s1 = app.local_adjust_page_layers[&0].clone();
            let mut pause = WritePause::next(&mut app);
            if completion_first {
                app.persist_local_adjust_shape_key_edit();
                pause.wait();
                pause.release();
                app.finish_local_adjust_writes();
                assert!(app.local_adjust_write_pending.is_empty());
                assert_saved(&app, &target, &s1);
            }

            let after = app
                .with_viewer_context(parked, |owner| {
                    assert_unfinished(owner, tool);
                    assert!(owner.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));
                    let run = owner.local_adjust_shape_key_edit.as_ref().unwrap();
                    assert_eq!(
                        run.before, s1,
                        "unfinished geometry must not retain obsolete S0 as the next key before"
                    );
                    assert_unfinished(owner, tool);
                    let mut expected = s1.clone();
                    let LocalMask::RasterVector(mask) = &mut Arc::make_mut(&mut expected)[0].mask
                    else {
                        unreachable!()
                    };
                    let local_adjust_core::MaskShape::Ellipse { center, .. } = &mut mask.shapes[0]
                    else {
                        unreachable!()
                    };
                    center[0] += 0.125;
                    assert_eq!(owner.local_adjust_page_layers[&0], expected);
                    expected
                })
                .unwrap();
            if !completion_first {
                pause.wait();
                assert_saved(&app, &target, &s0);
                pause.release();
                app.finish_local_adjust_writes();
                assert_saved(&app, &target, &s1);
            }
            app.with_viewer_context(parked, |owner| {
                assert_eq!(
                    owner.local_adjust_page_layers[&0], after,
                    "old completion cannot replace the active key draft"
                );
                assert_unfinished(owner, tool);
                owner.commit_local_adjust_pending_edits();
            })
            .unwrap();
            app.finish_local_adjust_writes();
            assert_saved(&app, &target, &after);
            assert_eq!(app.meta_undo.undo_len(), 2);
        }
    }
}

fn lut_document(name: &str) -> LocalAdjustmentLayers {
    let mut layers = document(name);
    Arc::make_mut(&mut layers)[0].effect = LocalEffect::CubeLut(Default::default());
    layers
}

fn ready_lut(name: &str) -> local_adjust_core::CubeLutParams {
    local_adjust_core::CubeLutParams {
        name: name.to_owned(),
        size: 2,
        table: vec![[0.25, 0.5, 0.75]; 8],
        ..Default::default()
    }
}

fn pending_lut(
    app: &mut App,
    idx: usize,
) -> std::sync::mpsc::Sender<Result<local_adjust_core::CubeLutParams, String>> {
    assert!(app.begin_local_page_edit(idx));
    let owner = app.capture_page_edit_owner(idx).unwrap();
    let document = app.local_page_edit_layers_for_target(&owner.target);
    let (tx, rx) = std::sync::mpsc::channel();
    app.local_adjust_lut_pending = Some(LocalAdjustLutLoadPending {
        owner,
        fs_idx: idx,
        layer_idx: 0,
        document,
        path: PathBuf::from("ready-fixture.cube"),
        rx,
    });
    tx
}

#[test]
fn ready_lut_keeps_original_page_target_after_switching_editor_document() {
    let (mut app, _) = pages(&["a", "b"]);
    let target_a = app.page_adjustment_target_for_idx(0).unwrap();
    let target_b = app.page_adjustment_target_for_idx(1).unwrap();
    let initial_a = lut_document("a-lut");
    app.set_local_adjust_layers_for_target(&target_a, initial_a.clone());
    app.finish_local_adjust_writes();
    let tx = pending_lut(&mut app, 0);
    let lut = ready_lut("original A result");
    tx.send(Ok(lut.clone())).unwrap();

    let ctx = egui::Context::default();
    // The switch entry owns a real spread pivot and prepares its target before
    // changing documents. Keep this LUT-delivery test on the ready path.
    app.local_adjust_spread_ctx = app.capture_local_page_edit_pivot(PageEditSpreadPivot {
        saved_mode: crate::settings::SpreadMode::Ltr,
        pair: (0, 1),
        navigation_anchor_idx: 0,
    });
    let pixels = egui::ColorImage::filled([1, 1], egui::Color32::GRAY);
    app.fs_cache.insert(
        1,
        FsCacheEntry::Static {
            pdf_rerender_failures: Vec::new(),
            pdf_metadata: None,
            tex: ctx.load_texture(
                "lut-switch-ready",
                pixels.clone(),
                egui::TextureOptions::LINEAR,
            ),
            pixels: Arc::new(pixels),
            source_dims: Some([1, 1]),
            load_seq: 0,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    app.switch_local_adjust_target_in_spread(&ctx, 1);
    assert_eq!(app.fullscreen_idx, Some(1));
    app.local_adjust_selected_shape = Some(0);
    app.select_local_page_edit_layer(1, 0);
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(1, 0.125, 0.0));
    let edited_b = app.local_adjust_page_layers[&1].clone();
    assert!(matches!(&app.local_page_edit_document,
        LocalPageEditDocument::Editing { owner, .. } if owner.target.page_key == target_b.page_key));
    assert!(app.local_adjust_lut_pending.is_some());

    app.poll_local_adjust_lut_load(&egui::Context::default());
    assert!(app.local_adjust_lut_pending.is_none());
    let mut expected_a = initial_a;
    Arc::make_mut(&mut expected_a)[0].effect = LocalEffect::CubeLut(lut);
    assert_eq!(app.local_adjust_page_layers[&0], expected_a);
    assert_eq!(app.local_adjust_page_layers[&1], edited_b);
    app.commit_local_adjust_pending_edits();
    app.finish_local_adjust_writes();
    assert_saved(&app, &target_a, &expected_a);
    assert_saved(&app, &target_b, &edited_b);
}

#[cfg(windows)]
#[test]
fn single_bundle_waits_for_parked_lut_producer_and_its_write_before_replacing_page() {
    let (mut app, paths) = pages(&["shared"]);
    let target = app.page_adjustment_target_for_idx(0).unwrap();
    let s0 = lut_document("producer source");
    app.set_local_adjust_layers_for_target(&target, s0.clone());
    app.finish_local_adjust_writes();
    let tx = pending_lut(&mut app, 0);
    let parked = app.stash_mounted_and_start_fresh("producer_bundle_fence_test");
    install(&mut app, &paths, std::slice::from_ref(&s0));
    let s2 = document("bulk replacement");
    app.edit_bundle_clipboard = Some(crate::edit_bundle::EditBundleClipboard {
        source_label: "bulk replacement fixture".into(),
        bundle: crate::edit_bundle::PageEditBundle {
            source_size: [1, 1],
            local_adjust_layers: Some(s2.clone()),
            ..Default::default()
        },
    });
    let request = crate::edit_bundle::EditBundlePasteRequest {
        target_idx: 0,
        target_key: target.page_key.clone(),
        target_label: "shared page".into(),
        target_size: [1, 1],
        items_generation: app.items_generation,
        sidecar_coords: target.sidecar_coords.clone(),
    };
    app.start_apply_page_edit_bundle(request);
    assert!(
        app.has_page_edit_producers(),
        "the parked request participates in the fence"
    );
    assert!(app.edit_bundle_bulk_pending.is_some());
    assert!(
        app.edit_bundle_apply_pending.is_none(),
        "the DB replacement cannot start yet"
    );
    let ctx = egui::Context::default();
    let pump = |app: &mut App| {
        ctx.run_pass(egui::RawInput::default(), |ctx| {
            app.show_bulk_page_edit_dialog(ctx);
            app.show_edit_bundle_paste_confirm_dialog(ctx);
        })
        .drop_without_applying_deltas();
    };
    pump(&mut app);
    assert!(app.has_page_edit_producers());
    assert_saved(&app, &target, &s0);

    let mut pause = WritePause::next(&mut app);
    let lut = ready_lut("parked result before bulk");
    tx.send(Ok(lut.clone())).unwrap();
    pump(&mut app);
    pause.wait();
    assert!(!app.has_page_edit_producers());
    assert!(
        app.edit_bundle_bulk_pending.is_some(),
        "producer delivery advances to its write fence"
    );
    assert!(app.edit_bundle_apply_pending.is_none());
    assert_saved(&app, &target, &s0);
    let mut s1 = s0;
    Arc::make_mut(&mut s1)[0].effect = LocalEffect::CubeLut(lut);
    app.with_viewer_context(parked, |owner| {
        assert_eq!(owner.local_adjust_page_layers[&0], s1);
        assert!(owner.local_adjust_lut_pending.is_none());
    })
    .unwrap();

    pause.release();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.edit_bundle_bulk_pending.is_some() || app.edit_bundle_apply_pending.is_some() {
        pump(&mut app);
        assert!(
            std::time::Instant::now() < deadline,
            "producer/write fence and real bundle worker must complete"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_saved(&app, &target, &s2);
    assert!(!app.has_page_edit_producers());
    assert!(
        tx.send(Ok(ready_lut("late old result"))).is_err(),
        "the consumed producer no longer owns a completion receiver"
    );
    app.poll_all_page_edit_producers(&ctx);
    app.poll_local_adjust_write_results();
    assert_saved(&app, &target, &s2);
    assert_eq!(app.local_adjust_page_layers[&0], s2);
    app.finish_local_adjust_writes();
}

#[test]
fn exact_reader_swap_aligns_edit_session_with_second_same_key_slot() {
    let (mut app, paths) = pages(&["a", "b"]);
    let target_a = app.page_adjustment_target_for_idx(0).unwrap();
    let target_b = app.page_adjustment_target_for_idx(1).unwrap();
    let before = app.local_adjust_page_layers[&0].clone();
    let sibling = app.local_adjust_page_layers[&1].clone();
    let ctx = egui::Context::default();
    let pixels = Arc::new(egui::ColorImage::filled([1, 1], egui::Color32::WHITE));
    let tex = ctx.load_texture(
        "page-edit-exact-reader",
        (*pixels).clone(),
        Default::default(),
    );
    let texture_id = tex.id();
    app.fs_cache.insert(
        0,
        FsCacheEntry::Static {
            pdf_rerender_failures: Vec::new(),
            pdf_metadata: None,
            tex,
            pixels: pixels.clone(),
            source_dims: Some([1, 1]),
            load_seq: 1372,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));
    let draft = app.local_adjust_page_layers[&0].clone();
    let replacement = vec![
        GridItem::Image(paths[0].clone()),
        GridItem::Image(paths[1].clone()),
        GridItem::Image(paths[0].clone()),
    ];
    let swap = app.prepare_rating_membership_items_swap(&replacement);
    assert!(swap.is_some());
    app.replace_search_view_items(replacement, vec![None; 3]);
    // Adoption projects all aliases and initially chooses the first slot. The
    // actual reader transfer resolves the same identity to the second alias.
    assert_eq!(app.local_adjust_shape_key_edit.as_ref().unwrap().fs_idx, 0);
    app.finish_rating_membership_viewer_swap(swap);

    assert_eq!(app.fullscreen_idx, Some(2));
    let session = app.local_adjust_shape_key_edit.as_ref().unwrap();
    assert_eq!(
        session.fs_idx, 2,
        "the session must follow the exact reader, not the first alias"
    );
    assert_eq!(session.owner.slots, [0, 2]);
    assert_eq!(session.before, before);
    let LocalPageEditDocument::Editing { owner, layers, .. } = &app.local_page_edit_document else {
        panic!("adoption must retain the owning draft");
    };
    assert_eq!(owner.target.page_key, target_a.page_key);
    assert_eq!(*layers, draft);
    assert_eq!(app.local_adjust_page_layers[&0], draft);
    assert_eq!(app.local_adjust_page_layers[&2], draft);
    let Some(FsCacheEntry::Static {
        tex, pixels: kept, ..
    }) = app.fs_cache.get(&2)
    else {
        panic!("the real retained reader transfer must move its paint resource");
    };
    assert_eq!(tex.id(), texture_id);
    assert!(Arc::ptr_eq(kept, &pixels));

    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(2, 0.125, 0.0));
    assert_eq!(
        app.local_adjust_shape_key_edit.as_ref().unwrap().before,
        before
    );
    assert_eq!(app.local_adjust_shape_key_edit.as_ref().unwrap().fs_idx, 2);
    assert_eq!(app.meta_undo.undo_len(), 0);
    let after = app.local_adjust_page_layers[&2].clone();
    app.commit_local_adjust_pending_edits();
    app.finish_local_adjust_writes();
    assert_eq!(app.meta_undo.undo_len(), 1);
    assert_saved(&app, &target_a, &after);
    assert_saved(&app, &target_b, &sibling);
}

fn assert_saved_adjustment(
    app: &App,
    target: &PageAdjustmentTarget,
    expected: &crate::adjustment::AdjustParams,
) {
    assert_eq!(
        app.adjustment_db
            .as_ref()
            .unwrap()
            .get_page_params(&target.page_key),
        Some(expected.clone())
    );
    let (folder, item_key) = target.sidecar_coords.as_ref().unwrap();
    assert_eq!(
        app.sidecars.get(folder).unwrap().items()[item_key]
            .adjust
            .as_ref(),
        Some(expected)
    );
}

#[cfg(windows)]
#[test]
fn normal_page_commit_publishes_same_key_to_next_context_before_capture_and_undo() {
    let (mut app, paths) = pages(&["shared"]);
    app.local_adjust_mode = false;
    let target = app.page_adjustment_target_for_idx(0).unwrap();
    let layers = app.local_adjust_page_layers[&0].clone();
    let mut s0 = crate::adjustment::AdjustParams::default();
    s0.brightness = 5.0;
    app.set_page_params(0, s0.clone());
    let sibling = app.build_window_context_for_test(137218, |owner| {
        install(owner, &paths, std::slice::from_ref(&layers));
        owner.local_adjust_mode = false;
        owner.adjustment_page_params.insert(0, s0.clone());
        owner
            .page_edit_snapshot
            .as_mut()
            .unwrap()
            .adjustment
            .insert(target.page_key.clone(), s0.clone());
    });
    let mut s1 = s0.clone();
    s1.brightness = 10.0;
    app.apply_adjustment_panel_edit_frame(
        0,
        AdjustScopeSelection::Page,
        s0.clone(),
        s1.clone(),
        true,
        true,
        false,
    );
    app.apply_adjustment_panel_edit_frame(
        0,
        AdjustScopeSelection::Page,
        s1.clone(),
        s1.clone(),
        false,
        false,
        false,
    );
    assert_saved_adjustment(&app, &target, &s1);

    let mut after = s1.clone();
    after.brightness = 20.0;
    app.with_viewer_context(sibling, |owner| {
        assert_eq!(
            owner.adjustment_page_params[&0], s1,
            "Committed publication must update the inactive same-key projection"
        );
        // Model a UI pass collected from its former S0. The actual frame
        // acceptance rebases operated parameters and captures the latest S1.
        owner.apply_adjustment_panel_edit_frame(
            0,
            AdjustScopeSelection::Page,
            s0.clone(),
            after.clone(),
            true,
            true,
            false,
        );
        assert_eq!(
            owner.adjustment_drag_session.as_ref().unwrap().before,
            Some(s1.clone())
        );
        owner.apply_adjustment_panel_edit_frame(
            0,
            AdjustScopeSelection::Page,
            after.clone(),
            after.clone(),
            false,
            false,
            false,
        );
        owner.apply_meta_undo();
        assert_eq!(owner.adjustment_page_params[&0], s1);
        assert_saved_adjustment(owner, &target, &s1);
        owner.apply_meta_redo();
        assert_eq!(owner.adjustment_page_params[&0], after);
        assert_saved_adjustment(owner, &target, &after);
    })
    .unwrap();
    assert_eq!(app.adjustment_page_params[&0], after);
    assert_eq!(app.meta_undo.undo_len(), 2);
    assert_eq!(app.meta_undo.redo_len(), 0);
}

fn cache_thumb_adjustments(app: &mut App, ctx: &egui::Context, edited_key: &str) {
    app.thumb_edit_preview_keys.insert(2, edited_key.to_owned());
    for idx in 0..3 {
        app.thumb_adjust_tex.insert(
            idx,
            ctx.load_texture(
                format!("page-owner-thumb-{idx}"),
                egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
                Default::default(),
            ),
        );
    }
}

#[cfg(windows)]
#[test]
fn parked_reindexed_page_terminal_invalidates_owner_and_representative_thumbs_only() {
    let (mut app, paths) = pages(&["edited", "representative", "unrelated"]);
    app.local_adjust_mode = false;
    let target = app.page_adjustment_target_for_idx(0).unwrap();
    let unrelated_key = app.page_adjustment_target_for_idx(2).unwrap().page_key;
    let before = crate::adjustment::AdjustParams::default();
    let mut after = before.clone();
    after.brightness = 20.0;
    app.apply_adjustment_panel_edit_frame(
        0,
        AdjustScopeSelection::Page,
        before,
        after.clone(),
        true,
        true,
        false,
    );
    let order = vec![paths[2].clone(), paths[0].clone(), paths[1].clone()];
    let replacement = order
        .iter()
        .cloned()
        .map(GridItem::Image)
        .collect::<Vec<_>>();
    let swap = app.prepare_rating_membership_items_swap(&replacement);
    assert!(swap.is_some());
    app.replace_search_view_items(replacement, vec![None; 3]);
    app.finish_rating_membership_viewer_swap(swap);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.adjustment_drag_session.as_ref().unwrap().fs_idx, 1);
    assert_eq!(
        app.adjustment_page_params[&1], after,
        "the terminal sees an already-previewed final value"
    );
    let ctx = egui::Context::default();
    cache_thumb_adjustments(&mut app, &ctx, &target.page_key);
    let parked = app.stash_mounted_and_start_fresh("page_terminal_thumb_reindex_test");
    install(
        &mut app,
        &order,
        &[
            document("unrelated"),
            document("edited"),
            document("representative"),
        ],
    );
    app.local_adjust_mode = false;
    cache_thumb_adjustments(&mut app, &ctx, &target.page_key);
    app.prepare_page_edit_history_boundary();

    assert_saved_adjustment(&app, &target, &after);
    assert!(
        app.thumb_adjust_tex.contains_key(&0),
        "unrelated mounted thumbnail is retained"
    );
    assert!(!app.thumb_adjust_tex.contains_key(&1));
    assert!(!app.thumb_adjust_tex.contains_key(&2));
    assert!(
        app.pinned_adjustment_refresh_keys
            .contains(&target.page_key)
    );
    assert!(!app.pinned_adjustment_refresh_keys.contains(&unrelated_key));
    app.with_viewer_context(parked, |owner| {
        assert!(owner.adjustment_drag_session.is_none());
        assert!(
            owner.thumb_adjust_tex.contains_key(&0),
            "unrelated parked thumbnail is retained"
        );
        assert!(
            !owner.thumb_adjust_tex.contains_key(&1),
            "the edited page follows its stable key to slot 1"
        );
        assert!(
            !owner.thumb_adjust_tex.contains_key(&2),
            "the representative follows the edited key"
        );
        assert_eq!(owner.adjustment_page_params[&1], after);
    })
    .unwrap();
    assert_eq!(
        app.adjustment_db
            .as_ref()
            .unwrap()
            .get_page_params(&unrelated_key),
        None
    );
    assert_eq!(app.meta_undo.undo_len(), 1);
}

#[cfg(windows)]
#[test]
fn real_bundle_adjustment_commit_publishes_to_inactive_same_key_context() {
    let (mut app, paths) = pages(&["shared"]);
    app.local_adjust_mode = false;
    let target = app.page_adjustment_target_for_idx(0).unwrap();
    let layers = app.local_adjust_page_layers[&0].clone();
    let mut before = crate::adjustment::AdjustParams::default();
    before.brightness = 5.0;
    app.set_page_params(0, before.clone());
    let sibling = app.build_window_context_for_test(137219, |owner| {
        install(owner, &paths, std::slice::from_ref(&layers));
        owner.local_adjust_mode = false;
        owner.adjustment_page_params.insert(0, before.clone());
        owner
            .page_edit_snapshot
            .as_mut()
            .unwrap()
            .adjustment
            .insert(target.page_key.clone(), before.clone());
    });
    let mut after = before.clone();
    after.brightness = 30.0;
    app.edit_bundle_clipboard = Some(crate::edit_bundle::EditBundleClipboard {
        source_label: "normal adjustment publication fixture".into(),
        bundle: crate::edit_bundle::PageEditBundle {
            source_size: [1, 1],
            adjust: Some(after.clone()),
            local_adjust_layers: Some(layers),
            ..Default::default()
        },
    });
    app.with_viewer_context(sibling, |owner| {
        owner.start_apply_page_edit_bundle(crate::edit_bundle::EditBundlePasteRequest {
            target_idx: 0,
            target_key: target.page_key.clone(),
            target_label: "shared page".into(),
            target_size: [1, 1],
            items_generation: owner.items_generation,
            sidecar_coords: target.sidecar_coords.clone(),
        });
        assert!(owner.edit_bundle_apply_pending.is_some());
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while owner.edit_bundle_apply_pending.is_some() || owner.edit_bundle_bulk_pending.is_some()
        {
            ctx.run_pass(egui::RawInput::default(), |ctx| {
                owner.show_bulk_page_edit_dialog(ctx);
                owner.show_edit_bundle_paste_confirm_dialog(ctx);
            })
            .drop_without_applying_deltas();
            assert!(
                std::time::Instant::now() < deadline,
                "real bundle worker must deliver its publication"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(owner.adjustment_page_params[&0], after);
        assert_saved_adjustment(owner, &target, &after);
    })
    .unwrap();
    assert_eq!(
        app.adjustment_page_params[&0], after,
        "atomic bundle success must update the inactive same-key viewer"
    );
    assert_eq!(
        app.page_edit_snapshot.as_ref().unwrap().adjustment[&target.page_key],
        after
    );
    let mut next = after.clone();
    next.brightness = 40.0;
    app.apply_adjustment_panel_edit_frame(
        0,
        AdjustScopeSelection::Page,
        before,
        next.clone(),
        true,
        true,
        false,
    );
    assert_eq!(
        app.adjustment_drag_session.as_ref().unwrap().before,
        Some(after.clone())
    );
    app.apply_adjustment_panel_edit_frame(
        0,
        AdjustScopeSelection::Page,
        next.clone(),
        next,
        false,
        false,
        false,
    );
    app.apply_meta_undo();
    assert_eq!(app.adjustment_page_params[&0], after);
    assert_saved_adjustment(&app, &target, &after);
}

#[cfg(windows)]
#[test]
fn mounted_page_commit_refreshes_parked_container_representatives_before_they_mount() {
    for kind in ["folder", "zip", "pdf"] {
        let (mut app, paths) = pages(&["edited", "unrelated"]);
        app.local_adjust_mode = false;
        let container = match kind {
            "folder" => GridItem::Folder(paths[0].parent().unwrap().to_path_buf()),
            "zip" => {
                let zip_path = app.tmp.path().join("representative.zip");
                app.items[0] = GridItem::ZipImage {
                    zip_path: zip_path.clone(),
                    entry_name: "page.png".into(),
                };
                GridItem::ZipFile(zip_path)
            }
            "pdf" => {
                let pdf_path = app.tmp.path().join("representative.pdf");
                app.items[0] = GridItem::PdfPage {
                    pdf_path: pdf_path.clone(),
                    page_num: 0,
                    content_type: None,
                };
                GridItem::PdfFile(pdf_path)
            }
            _ => unreachable!(),
        };
        // This is a materialized-thumbnail lifecycle fixture; no ZIP/PDF decoder is needed.
        app.page_edit_snapshot.as_mut().unwrap().order =
            Arc::new(page_edit_snapshot::PageKeyOrder::from_items(&app.items).unwrap());
        let target = app.page_adjustment_target_for_idx(0).unwrap();
        let ctx = egui::Context::default();
        let representative_pixels = Arc::new(egui::ColorImage::filled([1, 1], egui::Color32::GRAY));
        let unrelated_pixels = Arc::new(egui::ColorImage::filled([1, 1], egui::Color32::WHITE));
        let unrelated_texture = ctx.load_texture(
            format!("unrelated-container-{kind}"),
            unrelated_pixels.as_ref().clone(),
            Default::default(),
        );
        let mounted = app.projected_viewer_context_id();
        let parked = app.build_window_context_for_test(137220, |owner| {
            owner.items = vec![container, GridItem::Image(paths[1].clone())];
            owner.image_metas = vec![None; 2];
            owner.visible_indices = vec![0, 1];
            owner.fullscreen_idx = None;
            owner.selected = None;
            owner.local_adjust_mode = false;
            owner.thumbnails = vec![
                ThumbnailState::loaded_raster(
                    ctx.load_texture(
                        format!("baked-container-{kind}"),
                        representative_pixels.as_ref().clone(),
                        Default::default(),
                    ),
                    crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                        evaluated_display_px: 1,
                    },
                    true,
                    1,
                    crate::source_coverage::SourceCoverage::FullSource,
                    Some((1, 1)),
                    None,
                ),
                ThumbnailState::loaded_raster(
                    unrelated_texture.clone(),
                    crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                        evaluated_display_px: 1,
                    },
                    false,
                    1,
                    crate::source_coverage::SourceCoverage::FullSource,
                    Some((1, 1)),
                    None,
                ),
            ];
            owner.thumb_pixels.insert(0, representative_pixels.clone());
            owner.thumb_pixels.insert(1, unrelated_pixels.clone());
            owner
                .thumb_edit_preview_keys
                .insert(0, target.page_key.clone());
            owner.requested.insert(0, false);
            owner.requested.insert(1, false);
            assert!(!owner.items[0].has_page_data());
            assert!(
                (0..owner.items.len()).all(
                    |idx| owner.page_path_key(idx).as_deref() != Some(target.page_key.as_str())
                ),
                "the parked context contains the representative, never the edited page"
            );
        });
        assert_eq!(app.projected_viewer_context_id(), mounted);
        let before = crate::adjustment::AdjustParams::default();
        let mut after = before.clone();
        after.brightness = 20.0;
        app.apply_adjustment_panel_edit_frame(
            0,
            AdjustScopeSelection::Page,
            before,
            after.clone(),
            true,
            true,
            false,
        );
        app.apply_adjustment_panel_edit_frame(
            0,
            AdjustScopeSelection::Page,
            after.clone(),
            after.clone(),
            false,
            false,
            false,
        );
        assert_saved_adjustment(&app, &target, &after);
        assert_eq!(app.meta_undo.undo_len(), 1);
        assert!(
            app.pinned_adjustment_refresh_keys
                .contains(&target.page_key)
        );

        // A consumes the App-global notification while B remains at rest. B must receive it
        // now, rather than depending on a later mount finding an already-drained key set.
        app.reconcile_pinned_adjustment_refreshes();
        assert!(app.pinned_adjustment_refresh_keys.is_empty());
        assert_eq!(app.projected_viewer_context_id(), mounted);
        app.with_viewer_context(parked, |owner| {
            assert!(
                matches!(owner.thumbnails[0], ThumbnailState::Evicted),
                "{kind} representative must discard its baked texture before mounting"
            );
            assert!(!owner.thumb_pixels.contains_key(&0), "{kind} baked pixels");
            assert!(!owner.requested.contains_key(&0), "{kind} stale request");
            assert!(
                matches!(&owner.thumbnails[1], ThumbnailState::Loaded { tex, .. }
                    if tex.id() == unrelated_texture.id()),
                "unrelated texture remains materialized"
            );
            assert!(Arc::ptr_eq(&owner.thumb_pixels[&1], &unrelated_pixels));
            assert!(owner.requested.contains_key(&1));
        })
        .unwrap();
    }
}

fn assert_deleted(app: &App, target: &PageAdjustmentTarget) {
    assert!(
        app.local_adjust_db
            .as_ref()
            .unwrap()
            .get_layers(&target.page_key)
            .is_none()
    );
    let (folder, rel) = target.sidecar_coords.as_ref().unwrap();
    assert!(
        app.sidecars
            .get(folder)
            .unwrap()
            .items()
            .get(rel)
            .and_then(|item| item.local_adjust_layers.as_ref())
            .is_none()
    );
}

#[cfg(windows)]
fn foreign_delete_after_paused_write(nonresident_origin: bool) {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let (mut app, paths) = pages(&["shared", "unrelated"]);
    let target = app.page_adjustment_target_for_idx(0).unwrap();
    let unrelated_target = app.page_adjustment_target_for_idx(1).unwrap();
    let unrelated = app.local_adjust_page_layers[&1].clone();
    let initial = app.local_adjust_page_layers[&0].clone();
    let stamp = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.sample();
    app.page_edit_snapshot.as_mut().unwrap().stamp = Some(stamp);
    let sibling = app.build_window_context_for_test(1372131, |owner| {
        install(owner, &paths, &[initial.clone(), unrelated.clone()]);
        let snapshot = owner.page_edit_snapshot.as_mut().unwrap();
        snapshot
            .local_adjust
            .extend([target.page_key.clone(), unrelated_target.page_key.clone()]);
        snapshot.stamp = Some(stamp);
    });
    let mut first_pause = WritePause::next(&mut app);
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));
    let accepted_a = app.local_adjust_page_layers[&0].clone();
    app.persist_local_adjust_shape_key_edit();
    first_pause.wait();
    assert_saved(&app, &target, &initial);
    if nonresident_origin {
        // An accepted request outlives its resident slot. The context still
        // owns its captured key, while the projected order contains only U.
        install(&mut app, &paths[1..], std::slice::from_ref(&unrelated));
        app.reproject_page_edit_owners();
        app.page_edit_snapshot.as_mut().unwrap().stamp = Some(stamp);
        app.refresh_pending_page_edit_projections();
        assert!(
            app.page_edit_snapshot
                .as_ref()
                .unwrap()
                .local_adjust
                .contains(&target.page_key)
        );
        assert!(
            app.page_edit_snapshot
                .as_ref()
                .unwrap()
                .volatile_keys
                .contains(&target.page_key)
        );
    }
    app.with_viewer_context(sibling, |owner| {
        assert_eq!(owner.local_adjust_page_layers[&0], accepted_a);
        owner.clear_local_adjust_layers_from_panel_for_test(0);
        assert!(owner.local_adjust_shape_key_edit.is_none());
        assert!(!owner.local_adjust_page_layers.contains_key(&0));
        assert!(
            !owner
                .page_edit_snapshot
                .as_ref()
                .unwrap()
                .local_adjust
                .contains(&target.page_key)
        );
        assert!(
            owner
                .page_edit_snapshot
                .as_ref()
                .unwrap()
                .volatile_keys
                .contains(&target.page_key)
        );
        assert_eq!(owner.local_adjust_page_layers[&1], unrelated);
    })
    .unwrap();
    assert!(
        !app.page_edit_snapshot
            .as_ref()
            .unwrap()
            .local_adjust
            .contains(&target.page_key)
    );
    assert!(
        app.page_edit_snapshot
            .as_ref()
            .unwrap()
            .volatile_keys
            .contains(&target.page_key)
    );
    let mut second_pause = WritePause::next(&mut app);
    first_pause.release();
    second_pause.wait();
    app.poll_local_adjust_write_results();
    assert_saved(&app, &target, &accepted_a);
    assert!(
        app.local_adjust_write_pending
            .contains_key(&target.page_key)
    );
    // A's older success must not restore presence or clear the latest B hold.
    assert!(
        !app.page_edit_snapshot
            .as_ref()
            .unwrap()
            .local_adjust
            .contains(&target.page_key)
    );
    assert!(
        app.page_edit_snapshot
            .as_ref()
            .unwrap()
            .volatile_keys
            .contains(&target.page_key)
    );
    let before_metadata_completion = app.page_edit_revision;
    second_pause.release();
    app.finish_local_adjust_writes();
    assert_deleted(&app, &target);
    assert_saved(&app, &unrelated_target, &unrelated);
    assert!(
        app.page_edit_revision > before_metadata_completion,
        "metadata-only volatile release must invalidate the old read fence"
    );
    assert!(
        !app.page_edit_snapshot
            .as_ref()
            .unwrap()
            .local_adjust
            .contains(&target.page_key)
    );
    assert!(
        !app.page_edit_snapshot
            .as_ref()
            .unwrap()
            .volatile_keys
            .contains(&target.page_key)
    );
    app.with_viewer_context(sibling, |owner| {
        assert!(
            !owner
                .page_edit_snapshot
                .as_ref()
                .unwrap()
                .local_adjust
                .contains(&target.page_key)
        );
        assert!(
            !owner
                .page_edit_snapshot
                .as_ref()
                .unwrap()
                .volatile_keys
                .contains(&target.page_key)
        );
        assert!(!owner.local_adjust_pages.contains(&0));
        assert_eq!(owner.local_adjust_page_layers[&1], unrelated);
    })
    .unwrap();
    // Force the legitimate earlier read stamp, then drive the real async
    // reconciliation handler through its worker and adoption, not a helper
    // that installs a pre-cleared synthetic projection.
    app.page_edit_snapshot.as_mut().unwrap().stamp = Some(stamp);
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        app.poll_page_edit_reconciliation(&ctx);
        let settled = app.page_edit_reconcile_pending.is_none()
            && app
                .page_edit_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.stamp)
                .is_some_and(|stamp| crate::page_edit_write_epoch::PAGE_EDIT_WRITES.accepts(stamp));
        if settled {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "A reconciliation must complete"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        !app.page_edit_snapshot
            .as_ref()
            .unwrap()
            .local_adjust
            .contains(&target.page_key)
    );
    assert!(
        !app.page_edit_snapshot
            .as_ref()
            .unwrap()
            .volatile_keys
            .contains(&target.page_key)
    );
    assert!(!app.local_adjust_page_keys.contains(&target.page_key));
    let unrelated_idx = usize::from(!nonresident_origin);
    if !nonresident_origin {
        assert!(!app.local_adjust_pages.contains(&0));
        assert!(!app.local_adjust_page_layers.contains_key(&0));
    }
    assert_eq!(app.local_adjust_page_layers[&unrelated_idx], unrelated);
}

#[cfg(windows)]
#[test]
fn foreign_delete_releases_older_owner_volatile_and_reconcile_keeps_last_layer_absent() {
    foreign_delete_after_paused_write(false);
}

#[cfg(windows)]
#[test]
fn foreign_delete_updates_nonresident_owner_snapshot_before_reconciliation() {
    foreign_delete_after_paused_write(true);
}

#[cfg(windows)]
#[test]
fn foreign_delete_completion_preserves_same_key_active_draft_and_its_volatile() {
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let (mut app, paths) = pages(&["shared"]);
    let target = app.page_adjustment_target_for_idx(0).unwrap();
    let initial = app.local_adjust_page_layers[&0].clone();
    let sibling = app.build_window_context_for_test(1372132, |owner| {
        install(owner, &paths, std::slice::from_ref(&initial));
    });
    let mut pause = WritePause::next(&mut app);
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));
    app.persist_local_adjust_shape_key_edit();
    pause.wait();
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(0, 0.125, 0.0));
    let active = app.local_adjust_page_layers[&0].clone();
    app.with_viewer_context(sibling, |owner| {
        // A later asynchronous/API receipt may arrive while A has a new live
        // edit. It does not accept a history action that would finish A first.
        owner.set_local_adjust_layers_for_target(&target, LocalAdjustmentLayers::default());
    })
    .unwrap();
    assert_eq!(app.local_adjust_page_layers[&0], active);
    pause.release();
    app.finish_local_adjust_writes();
    assert_deleted(&app, &target);
    assert_eq!(app.local_adjust_page_layers[&0], active);
    assert!(app.local_adjust_shape_key_edit.is_some());
    assert!(
        app.page_edit_snapshot
            .as_ref()
            .unwrap()
            .local_adjust
            .contains(&target.page_key)
    );
    assert!(
        app.page_edit_snapshot
            .as_ref()
            .unwrap()
            .volatile_keys
            .contains(&target.page_key)
    );
    app.with_viewer_context(sibling, |owner| {
        assert!(
            !owner
                .page_edit_snapshot
                .as_ref()
                .unwrap()
                .local_adjust
                .contains(&target.page_key)
        );
        assert!(
            !owner
                .page_edit_snapshot
                .as_ref()
                .unwrap()
                .volatile_keys
                .contains(&target.page_key)
        );
    })
    .unwrap();
}
