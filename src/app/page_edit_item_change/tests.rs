use super::*;
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::settings::SpreadMode;

fn fixture() -> AppTestEnv {
    let mut env = setup_app();
    let folder = env.tmp.path().join("pages");
    std::fs::create_dir_all(&folder).unwrap();
    let paths = (0..3)
        .map(|n| folder.join(format!("{n}.png")))
        .collect::<Vec<_>>();
    for (n, path) in paths.iter().enumerate() {
        image::RgbaImage::from_pixel(4 + n as u32, 3, image::Rgba([80, 90, 100, 255]))
            .save(path)
            .unwrap();
    }
    env.items = paths.into_iter().map(GridItem::Image).collect();
    env.thumbnails = vec![ThumbnailState::Pending; 3];
    env.image_metas = vec![None; 3];
    env.visible_indices = vec![0, 1, 2];
    env.current_folder = Some(folder);
    env.fullscreen_idx = Some(1);
    env.selected = Some(1);
    env.spread_mode = SpreadMode::Single;
    env.settings.sidecar_backup_enabled = false;
    env
}

fn conceal(app: &mut App) {
    app.conceal_mode = true;
    app.conceal_mask_size = [5, 3];
    app.conceal_mask = Some(vec![true; 15]);
}

fn pivot() -> PageEditSpreadPivot {
    PageEditSpreadPivot {
        saved_mode: SpreadMode::Ltr,
        pair: (0, 1),
        navigation_anchor_idx: 0,
    }
}

fn open(app: &mut App, idx: usize) -> ActionAdmission {
    app.open_fullscreen_with_metadata_boundary_inner(
        idx,
        HistoryTrigger::UserChosen,
        FsOpenMaterialization::Eager,
        FsPageLoadContract::Sequential,
        None,
        FsMetadataBoundary::Manual,
        None,
        ViewerOpenAdoption::ContextContinuation,
        None,
    )
}

fn assert_saved(app: &App, key: &str) {
    let (bitmap, _, size) = app
        .conceal_db
        .as_ref()
        .unwrap()
        .get_full_checked(key)
        .unwrap()
        .expect("outgoing conceal must be saved");
    assert_eq!(size, [5, 3], "save must not rescale against the new image");
    assert_eq!(bitmap, vec![true; 15]);
}

fn assert_pivots_consumed(app: &App) {
    assert!(app.erase_spread_ctx.is_none());
    assert!(app.conceal_spread_ctx.is_none());
    assert!(app.text_spread_ctx.is_none());
    assert!(app.local_adjust_spread_ctx.is_none());
    assert!(app.export_crop_spread_ctx.is_none());
    assert!(app.sns_split_spread_ctx.is_none());
}

struct PreviewSaveTrace;

impl PreviewSaveTrace {
    fn start() -> Self {
        PREVIEW_SAVE_COUNT.with(|count| {
            assert!(count.replace(Some(0)).is_none());
        });
        Self
    }

    fn count(&self) -> usize {
        PREVIEW_SAVE_COUNT.with(|count| count.get().unwrap())
    }
}

impl Drop for PreviewSaveTrace {
    fn drop(&mut self) {
        PREVIEW_SAVE_COUNT.with(|count| count.set(None));
    }
}

fn seed_local_preview(app: &mut App) {
    app.local_adjust_mode = true;
    app.local_adjust_pages.insert(1);
    app.image_metas[1] = Some((1, 5));
    app.settings.edit_preview_cache_enabled = true;
    app.edit_preview_cache =
        Some(crate::edit_preview_cache::EditPreviewCacheService::open().unwrap());
    let key = app.current_edit_result_key(1);
    app.edit_result_cache.insert(
        key,
        EditResultEntry::raster(
            Arc::new(egui::ColorImage::filled([5, 3], egui::Color32::RED)),
            None,
        ),
    );
}

#[test]
fn page_edit_920_close_owns_single_local_preview_and_preserves_rating_history_and_landing() {
    for boundary in [
        FsMetadataBoundary::Manual,
        FsMetadataBoundary::RatingMembership,
    ] {
        let mut app = fixture();
        seed_local_preview(&mut app);
        app.local_adjust_spread_ctx = app.capture_local_page_edit_pivot(pivot());
        app.selected = Some(2);
        let change = crate::undo_stack::RatingChange {
            path_key: app.rating_path_key(1).unwrap(),
            source_path: app.items[1].drag_source_path().unwrap().to_path_buf(),
            meta: None,
            xmp_target: None,
            before: 1,
            after: 2,
        };
        let undo = crate::undo_stack::UndoEntry::Rating {
            changes: vec![change.clone()],
            origin: None,
            summary: "retained undo".into(),
        };
        let redo = crate::undo_stack::UndoEntry::Rating {
            changes: vec![change],
            origin: None,
            summary: "retained redo".into(),
        };
        app.meta_undo.push(undo);
        app.meta_undo.push_redo(redo);
        let trace = PreviewSaveTrace::start();

        app.close_fullscreen_now_for_destination(boundary, FullscreenCloseDestination::CurrentList);

        assert_eq!(
            trace.count(),
            1,
            "close alone owns the completed edit-result preview"
        );
        assert_eq!(app.fullscreen_idx, None);
        assert!(!app.local_adjust_mode);
        assert_pivots_consumed(&app);
        if boundary == FsMetadataBoundary::RatingMembership {
            assert_eq!(app.meta_undo.undo_len(), 1);
            assert_eq!(app.meta_undo.redo_len(), 1);
            assert_eq!(
                app.meta_undo.peek_undo().unwrap().summary(),
                "retained undo"
            );
            assert_eq!(
                app.meta_undo.peek_redo().unwrap().summary(),
                "retained redo"
            );
            assert_eq!(
                app.selected,
                Some(2),
                "rating adoption owns its destination"
            );
        } else {
            assert_eq!(
                app.selected,
                Some(0),
                "manual close lands on the outgoing spread anchor"
            );
        }
    }
}

#[test]
fn page_edit_920_local_panel_exit_caches_completed_preview_once() {
    let mut app = fixture();
    seed_local_preview(&mut app);
    let trace = PreviewSaveTrace::start();
    app.exit_local_adjust_mode(LocalAdjustmentExitPreview::CacheCurrent);
    assert!(!app.local_adjust_mode);
    assert_eq!(trace.count(), 1);
}

#[test]
fn page_edit_920_rating_close_keeps_undo_redo_across_local_fold() {
    let mut app = fixture();
    let mut mask = local_adjust_core::RasterVectorMask::empty(5, 3);
    mask.shapes.push(local_adjust_core::MaskShape::Ellipse {
        op: local_adjust_core::ShapeOp::Add,
        center: [0.5, 0.5],
        rx: 0.125,
        ry: 0.125,
        rotation_rad: 0.0,
    });
    app.set_local_adjust_layers_for_idx_memory_only(
        1,
        local_adjust_core::LocalAdjustmentLayers::new(vec![
            local_adjust_core::LocalAdjustmentLayer::new(
                "layer",
                local_adjust_core::LocalMask::RasterVector(mask),
                local_adjust_core::LocalEffect::None,
            ),
        ]),
    );
    app.local_adjust_mode = true;
    app.local_adjust_selected_shape = Some(0);
    app.local_adjust_mask_edit_target = LocalAdjustMaskEditTarget::Base;
    app.select_local_page_edit_layer(1, 0);
    assert!(app.nudge_selected_local_adjust_shape_from_shortcut(1, 0.125, 0.0));
    assert!(app.local_adjust_shape_key_edit.is_some());
    let changes = vec![crate::undo_stack::RatingChange {
        path_key: app.rating_path_key(1).unwrap(),
        source_path: app.items[1].drag_source_path().unwrap().to_path_buf(),
        meta: None,
        xmp_target: None,
        before: 1,
        after: 2,
    }];
    app.meta_undo.push(crate::undo_stack::UndoEntry::Rating {
        changes: changes.clone(),
        origin: None,
        summary: "undo before fold".into(),
    });
    app.meta_undo
        .push_redo(crate::undo_stack::UndoEntry::Rating {
            changes,
            origin: None,
            summary: "redo before fold".into(),
        });
    app.close_fullscreen_now_for_destination(
        FsMetadataBoundary::RatingMembership,
        FullscreenCloseDestination::CurrentList,
    );
    assert!(app.local_adjust_shape_key_edit.is_none());
    assert_eq!(app.meta_undo.undo_len(), 1);
    assert_eq!(app.meta_undo.redo_len(), 1);
    assert_eq!(
        app.meta_undo.peek_undo().unwrap().summary(),
        "undo before fold"
    );
    assert_eq!(
        app.meta_undo.peek_redo().unwrap().summary(),
        "redo before fold"
    );
    app.finish_local_adjust_writes();
}

#[test]
fn page_edit_920_close_discards_text_crop_and_sns_pivots_after_capturing_grid_landing() {
    for tool in ["text", "crop", "sns"] {
        let mut app = fixture();
        app.selected = Some(2);
        match tool {
            "text" => {
                app.text_mode = true;
                app.text_spread_ctx = Some(pivot());
            }
            "crop" => {
                app.export_crop_mode = true;
                app.export_crop_spread_ctx = Some(pivot());
            }
            _ => {
                app.sns_split = Some(crate::sns_split::SnsSplitLayout::centered_max(
                    crate::sns_split::SnsTarget::X,
                    2,
                    [5, 3],
                ));
                app.sns_split_spread_ctx = Some(pivot());
            }
        }
        app.close_fullscreen_now_for_destination(
            FsMetadataBoundary::Manual,
            FullscreenCloseDestination::CurrentList,
        );
        assert_eq!(
            app.spread_mode,
            SpreadMode::Single,
            "{tool} keeps the released close setting"
        );
        assert_eq!(
            app.selected,
            Some(0),
            "{tool} lands using the pivot before discarding it"
        );
        assert_eq!(app.fullscreen_idx, None);
        assert_pivots_consumed(&app);
        app.reconcile_page_edit_spread_pivots();
        assert_eq!(app.fullscreen_idx, None);
    }
}

#[test]
fn page_edit_920_inner_saves_outgoing_conceal_before_new_page_without_rescale() {
    let mut env = fixture();
    let old = env.page_path_key(1).unwrap();
    let new = env.page_path_key(2).unwrap();
    conceal(&mut env);
    assert_eq!(open(&mut env, 2), ActionAdmission::Accepted);
    assert_eq!(env.fullscreen_idx, Some(2));
    assert!(!env.conceal_mode);
    assert_saved(&env, &old);
    assert!(
        env.conceal_db
            .as_ref()
            .unwrap()
            .get_full_checked(&new)
            .unwrap()
            .is_none()
    );
}

#[test]
fn page_edit_920_inner_spread_exits_conceal_and_erase_before_assigning_destination() {
    for erase in [false, true] {
        for destination in [0, 2] {
            let mut env = fixture();
            if erase {
                env.erase_mode = true;
                env.erase_mask = Some(vec![true; 15]);
                env.erase_mask_size = [5, 3];
                env.erase_spread_ctx = Some(pivot());
            } else {
                conceal(&mut env);
                env.conceal_spread_ctx = Some(pivot());
            }
            assert_eq!(open(&mut env, destination), ActionAdmission::Accepted);
            assert_eq!(env.fullscreen_idx, Some(destination));
            assert_eq!(env.spread_mode, SpreadMode::Ltr);
            assert!(!env.erase_mode && !env.conceal_mode);
            assert_pivots_consumed(&env);
            env.reconcile_page_edit_spread_pivots();
            assert_eq!(env.fullscreen_idx, Some(destination));
        }
    }
}

#[test]
fn page_edit_920_inner_local_pivot_cannot_rewind_destination_next_frame() {
    let mut env = fixture();
    env.local_adjust_mode = true;
    env.local_adjust_spread_ctx = env.capture_local_page_edit_pivot(pivot());
    assert!(env.local_adjust_spread_ctx.is_some());
    assert_eq!(open(&mut env, 2), ActionAdmission::Accepted);
    assert!(!env.local_adjust_mode);
    assert_eq!(env.spread_mode, SpreadMode::Ltr);
    env.reconcile_page_edit_spread_pivots();
    assert_eq!(env.fullscreen_idx, Some(2));
    assert_pivots_consumed(&env);
}

#[test]
fn page_edit_920_item_change_consumes_already_closed_local_panel_pivot_before_next_frame() {
    let mut env = fixture();
    env.local_adjust_spread_ctx = env.capture_local_page_edit_pivot(pivot());
    // The panel's ordinary exit turned the mode off; its frame reconciliation has not run.
    env.local_adjust_mode = false;
    assert!(!env.is_overlay_edit_mode_active());
    assert_eq!(open(&mut env, 2), ActionAdmission::Accepted);
    assert_eq!(env.spread_mode, SpreadMode::Ltr);
    assert_pivots_consumed(&env);
    env.reconcile_page_edit_spread_pivots();
    assert_eq!(env.fullscreen_idx, Some(2));
}

#[test]
fn page_edit_920_inner_same_image_keeps_five_tools_but_exits_erase_and_restores_spread() {
    for tool in 0..6 {
        let mut env = fixture();
        match tool {
            0 => conceal(&mut env),
            1 => {
                env.erase_mode = true;
                env.erase_mask = Some(vec![true; 15]);
                env.erase_mask_size = [5, 3];
                env.erase_spread_ctx = Some(pivot());
            }
            2 => {
                env.text_mode = true;
                env.text_selected = Some(920);
            }
            3 => env.local_adjust_mode = true,
            4 => env.export_crop_mode = true,
            _ => {
                env.sns_split = Some(crate::sns_split::SnsSplitLayout::centered_max(
                    crate::sns_split::SnsTarget::X,
                    2,
                    [5, 3],
                ))
            }
        }
        assert_eq!(open(&mut env, 1), ActionAdmission::Accepted, "tool={tool}");
        if tool == 1 {
            assert!(!env.erase_mode);
            assert!(env.erase_mask.is_none());
            assert!(env.erase_spread_ctx.is_none());
            assert_eq!(env.fullscreen_idx, Some(0));
            assert_eq!(env.spread_mode, SpreadMode::Ltr);
            env.reconcile_page_edit_spread_pivots();
            assert_eq!(env.fullscreen_idx, Some(0));
            continue;
        }
        assert!(env.is_overlay_edit_mode_active(), "tool={tool}");
        assert_eq!(env.fullscreen_idx, Some(1));
        if tool == 2 {
            assert_eq!(env.text_selected, Some(920));
        }
    }
}

#[test]
fn page_edit_920_inner_failed_destination_still_finishes_requested_item_change() {
    let mut env = fixture();
    let old = env.page_path_key(1).unwrap();
    conceal(&mut env);
    assert_eq!(open(&mut env, usize::MAX), ActionAdmission::Rejected);
    assert!(!env.conceal_mode);
    assert_saved(&env, &old);
    assert_eq!(env.fullscreen_idx, Some(1));
}

#[test]
fn page_edit_920_install_new_items_same_index_saves_old_identity_before_replacement() {
    let mut env = fixture();
    let old = env.page_path_key(1).unwrap();
    let new = env.tmp.path().join("replacement.png");
    let mut items = env.items.clone();
    items[1] = GridItem::Image(new);
    conceal(&mut env);
    // Common producer boundary used by accepted Stack/search/aggregate row replacements.
    env.install_new_items(items, vec![None; 3]);
    let new = env.page_path_key(1).unwrap();
    assert_ne!(old, new);
    assert_saved(&env, &old);
    assert!(
        env.conceal_db
            .as_ref()
            .unwrap()
            .get_full_checked(&new)
            .unwrap()
            .is_none()
    );
    assert!(!env.conceal_mode);
}

#[test]
fn page_edit_920_non_grid_batch_removal_consumes_old_index_spread_pivot() {
    let mut env = fixture();
    let old = env.page_path_key(1).unwrap();
    conceal(&mut env);
    env.conceal_spread_ctx = Some(pivot());
    env.remove_items_batch(&[0]);
    assert_saved(&env, &old);
    assert!(!env.conceal_mode);
    assert_eq!(env.items.len(), 2);
    assert_pivots_consumed(&env);
    let landing = env.fullscreen_idx;
    env.reconcile_page_edit_spread_pivots();
    assert_eq!(env.fullscreen_idx, landing);
}

#[test]
fn page_edit_920_close_saves_conceal_and_text_before_clearing_idx_and_pivots() {
    for text in [false, true] {
        for spread in [false, true] {
            let mut env = fixture();
            let old = env.page_path_key(1).unwrap();
            if text {
                env.text_mode = true;
                let object = comic_core::AnnotationObject::new_text(
                    920,
                    (1.0, 1.0),
                    comic_core::TextBlock {
                        text: "未保存".into(),
                        ..Default::default()
                    },
                );
                env.comic_docs.insert(old.clone(), vec![object.clone()]);
                env.text_dirty_at = Some(std::time::Instant::now());
                if spread {
                    env.text_spread_ctx = Some(pivot());
                }
            } else {
                conceal(&mut env);
                if spread {
                    env.conceal_spread_ctx = Some(pivot());
                }
            }
            env.close_fullscreen_now_for_destination(
                FsMetadataBoundary::Manual,
                FullscreenCloseDestination::CurrentList,
            );
            assert_eq!(env.fullscreen_idx, None);
            assert!(!env.is_overlay_edit_mode_active());
            assert_pivots_consumed(&env);
            if text {
                let objects = env.comic_db.as_ref().unwrap().get(&old).unwrap();
                assert_eq!(objects[0].text_block().unwrap().text, "未保存");
            } else {
                assert_saved(&env, &old);
            }
            env.reconcile_page_edit_spread_pivots();
            assert_eq!(env.fullscreen_idx, None);
        }
    }
}

#[test]
fn page_edit_920_snapshot_activate_and_deactivate_end_edits_before_index_capture() {
    let mut env = fixture();
    let old = env.page_path_key(1).unwrap();
    env.visible_indices = vec![1, 2];
    conceal(&mut env);
    env.conceal_spread_ctx = Some(pivot());
    env.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    assert!(!env.conceal_mode);
    assert_saved(&env, &old);
    let replaced_index_key = env.page_path_key(1).unwrap();
    assert_ne!(
        replaced_index_key, old,
        "snapshot is a real same-index replacement producer"
    );
    assert!(
        env.conceal_db
            .as_ref()
            .unwrap()
            .get_full_checked(&replaced_index_key)
            .unwrap()
            .is_none()
    );
    assert_pivots_consumed(&env);
    assert!(env.snapshot.is_some());
    env.fullscreen_idx = Some(0);
    conceal(&mut env);
    env.deactivate_snapshot();
    assert!(!env.conceal_mode);
    assert_pivots_consumed(&env);
    assert_saved(&env, &old);
}

#[test]
fn page_edit_920_close_consumes_all_six_tools_spread_pivots_synchronously() {
    for tool in 0..6 {
        let mut env = fixture();
        match tool {
            0 => {
                conceal(&mut env);
                env.conceal_spread_ctx = Some(pivot());
            }
            1 => {
                env.erase_mode = true;
                env.erase_spread_ctx = Some(pivot());
            }
            2 => {
                env.text_mode = true;
                env.text_spread_ctx = Some(pivot());
            }
            3 => {
                env.local_adjust_mode = true;
                env.local_adjust_spread_ctx = env.capture_local_page_edit_pivot(pivot());
            }
            4 => {
                env.export_crop_mode = true;
                env.export_crop_spread_ctx = Some(pivot());
            }
            _ => {
                env.sns_split = Some(crate::sns_split::SnsSplitLayout::centered_max(
                    crate::sns_split::SnsTarget::X,
                    2,
                    [5, 3],
                ));
                env.sns_split_spread_ctx = Some(pivot());
            }
        }
        env.close_fullscreen_now_for_destination(
            FsMetadataBoundary::Manual,
            FullscreenCloseDestination::CurrentList,
        );
        assert_eq!(env.fullscreen_idx, None, "tool={tool}");
        assert!(!env.is_overlay_edit_mode_active(), "tool={tool}");
        assert_pivots_consumed(&env);
        env.reconcile_page_edit_spread_pivots();
        assert_eq!(env.fullscreen_idx, None, "tool={tool}");
    }
}

#[cfg(windows)]
#[test]
fn page_edit_920_sibling_close_and_row_adoption_preserve_foreground_draft() {
    let mut env = fixture();
    let sibling_path = env.tmp.path().join("sibling.png");
    let sibling = env.build_window_context_for_test(1920920, |app| {
        app.items = vec![GridItem::Image(sibling_path)];
        app.thumbnails = vec![ThumbnailState::Pending];
        app.image_metas = vec![None];
        app.fullscreen_idx = Some(0);
    });
    let old = env.page_path_key(1).unwrap();
    conceal(&mut env);
    let sibling_key = env
        .with_viewer_context_state(sibling, |app| {
            let key = app.page_path_key(0).unwrap();
            app.install_new_items(vec![app.items[0].clone()], vec![None]);
            assert!(app.conceal_mode);
            app.close_fullscreen_now_for_destination(
                FsMetadataBoundary::Manual,
                FullscreenCloseDestination::CurrentList,
            );
            assert_eq!(app.fullscreen_idx, None);
            assert!(app.conceal_mode);
            key
        })
        .unwrap();
    assert!(env.conceal_mode);
    assert_eq!(env.fullscreen_idx, Some(1));
    assert_eq!(env.conceal_mask_size, [5, 3]);
    assert_eq!(env.conceal_mask.as_deref(), Some([true; 15].as_slice()));
    assert!(
        env.conceal_db
            .as_ref()
            .unwrap()
            .get_full_checked(&sibling_key)
            .unwrap()
            .is_none()
    );
    env.close_fullscreen_now_for_destination(
        FsMetadataBoundary::Manual,
        FullscreenCloseDestination::CurrentList,
    );
    assert_saved(&env, &old);
    assert_eq!(env.fullscreen_idx, None);
}

#[cfg(windows)]
#[test]
fn page_edit_920_foreground_exit_does_not_touch_parked_items_cache_generation_or_cancel() {
    let mut env = fixture();
    let ctx = egui::Context::default();
    let (_sender, receiver) = mpsc::channel();
    let pending_cancel = Arc::new(AtomicBool::new(false));
    let sibling = env.build_window_context_for_test(1920921, |app| {
        app.items = vec![GridItem::Image(PathBuf::from("sibling.png"))];
        app.thumbnails = vec![ThumbnailState::Pending];
        app.image_metas = vec![None];
        app.fullscreen_idx = Some(0);
        app.input_generation.insert(0, 920);
        let pixels = Arc::new(egui::ColorImage::filled([1, 1], egui::Color32::WHITE));
        let tex = ctx.load_texture("parked-page", pixels.clone(), Default::default());
        app.fs_cache.insert(
            0,
            FsCacheEntry::Static {
                pdf_rerender_failures: Vec::new(),
                pdf_metadata: None,
                tex,
                pixels,
                source_dims: Some([1, 1]),
                load_seq: 920,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.folder_nav_pending = Some(FolderNavPending {
            cancel: pending_cancel.clone(),
            rx: receiver,
            forward: true,
            mode: FolderNavMode::Fullscreen,
        });
    });
    let state = |app: &mut App| {
        (
            app.fullscreen_idx,
            app.items.iter().map(GridItem::perf_key).collect::<Vec<_>>(),
            app.items_generation,
            app.input_generation.clone(),
            app.fs_cache.len(),
            app.fs_pending.len(),
            app.cancel_token.clone(),
        )
    };
    let before = env.with_viewer_context_state(sibling, state).unwrap();
    conceal(&mut env);
    env.end_page_edit_tools_for_viewer_item_change("test");
    let replacement = env.items.clone();
    env.install_new_items(replacement, vec![None; 3]);
    let after = env.with_viewer_context_state(sibling, state).unwrap();
    assert_eq!(
        (
            &before.0, &before.1, &before.2, &before.3, &before.4, &before.5
        ),
        (&after.0, &after.1, &after.2, &after.3, &after.4, &after.5)
    );
    assert!(Arc::ptr_eq(&before.6, &after.6));
    assert!(!after.6.load(Ordering::Relaxed));
    assert!(!pending_cancel.load(Ordering::Relaxed));
    assert!(
        env.with_viewer_context_state(sibling, |app| app.folder_nav_pending.is_some())
            .unwrap()
    );
}
