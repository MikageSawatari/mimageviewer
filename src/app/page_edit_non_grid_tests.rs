//! §1.920: production continuations and non-grid row adoption keep the outgoing owner.

use super::*;
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::settings::SpreadMode;

fn fixture(ctx: &egui::Context) -> AppTestEnv {
    let mut env = setup_app();
    let folder = env.tmp.path().join("non-grid-pages");
    std::fs::create_dir_all(&folder).unwrap();
    let paths = (0..3)
        .map(|n| folder.join(format!("{n}.png")))
        .collect::<Vec<_>>();
    for (idx, path) in paths.iter().enumerate() {
        let width = 4 + idx;
        image::RgbaImage::from_pixel(width as u32, 3, image::Rgba([80, 90, 100, 255]))
            .save(path)
            .unwrap();
        let pixels = Arc::new(egui::ColorImage::filled([width, 3], egui::Color32::WHITE));
        env.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                pdf_rerender_failures: Vec::new(),
                pdf_metadata: None,
                tex: ctx.load_texture(
                    format!("non-grid-{idx}"),
                    pixels.clone(),
                    Default::default(),
                ),
                pixels,
                source_dims: Some([width, 3]),
                load_seq: 0,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
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

fn pivot() -> PageEditSpreadPivot {
    PageEditSpreadPivot {
        saved_mode: SpreadMode::Ltr,
        pair: (0, 1),
        navigation_anchor_idx: 0,
    }
}

fn conceal(app: &mut App) {
    app.conceal_mode = true;
    app.conceal_mask_size = [5, 3];
    app.conceal_mask = Some(vec![true; 15]);
    app.conceal_spread_ctx = Some(pivot());
}

fn assert_saved(app: &App, key: &str) {
    let (bitmap, _, size) = app
        .conceal_db
        .as_ref()
        .unwrap()
        .get_full_checked(key)
        .unwrap()
        .expect("the outgoing draft must be saved");
    assert_eq!(size, [5, 3]);
    assert_eq!(bitmap, vec![true; 15]);
}

#[test]
fn page_edit_920_sidecar_deferred_open_finishes_old_edit_before_wait_and_never_rewinds() {
    for erase in [false, true] {
        let ctx = egui::Context::default();
        let mut app = fixture(&ctx);
        let old = app.page_path_key(1).unwrap();
        let new = app.page_path_key(2).unwrap();
        if erase {
            app.erase_mode = true;
            app.erase_mask_size = [5, 3];
            app.erase_mask = Some(vec![true; 15]);
            app.erase_spread_ctx = Some(pivot());
        } else {
            conceal(&mut app);
        }
        let folder = app.current_folder.clone().unwrap();
        let _thumb_rx = app.activate_live_sidecar_restore_for_test(folder);
        assert_eq!(
            app.open_fullscreen_with_admission(2, HistoryTrigger::UserChosen),
            ActionAdmission::Accepted
        );
        assert!(app.sidecar_restore_deferred_fullscreen_wait_active());
        assert!(!app.erase_mode && !app.conceal_mode);
        assert!(app.erase_spread_ctx.is_none() && app.conceal_spread_ctx.is_none());
        assert_eq!(
            app.fullscreen_idx,
            Some(0),
            "the edit exits before the deferred target opens"
        );
        assert_eq!(app.spread_mode, SpreadMode::Ltr);
        if !erase {
            assert_saved(&app, &old);
        }
        app.finish_page_identity_preparation_for_test();
        assert_eq!(app.fullscreen_idx, Some(2));
        assert!(!app.is_overlay_edit_mode_active());
        app.reconcile_page_edit_spread_pivots();
        assert_eq!(app.fullscreen_idx, Some(2));
        assert!(
            app.conceal_db
                .as_ref()
                .unwrap()
                .get_full_checked(&new)
                .unwrap()
                .is_none()
        );
        if !erase {
            assert_saved(&app, &old);
        }
    }
}

#[test]
fn page_edit_920_book_append_undo_completion_ends_edit_before_non_grid_removal() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let old = app.page_path_key(1).unwrap();
    let removed = app.items[0].drag_source_path().unwrap().to_path_buf();
    let remaining = app.items[1].clone();
    let folder = app.current_folder.clone().unwrap();
    conceal(&mut app);
    app.finish_book_append_undo(crate::book_append_undo::BookAppendUndoResult {
        book_name: "regression book".into(),
        folder,
        succeeded: vec![removed],
        failed: Vec::new(),
        purge_errors: Vec::new(),
        store_mutations: Default::default(),
    });
    assert_eq!(app.items.len(), 2);
    assert_eq!(app.items[0], remaining);
    assert!(!app.conceal_mode);
    assert!(app.conceal_spread_ctx.is_none());
    assert_eq!(app.spread_mode, SpreadMode::Ltr);
    assert_saved(&app, &old);
    let landed = app.fullscreen_idx;
    app.reconcile_page_edit_spread_pivots();
    assert_eq!(app.fullscreen_idx, landed);
}

#[test]
fn page_edit_920_direct_reader_adoption_exits_before_capturing_return_presentation() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let old = app.page_path_key(1).unwrap();
    let new_path = app.current_folder.as_ref().unwrap().join("excluded.png");
    image::RgbaImage::from_pixel(7, 3, image::Rgba([10, 20, 30, 255]))
        .save(&new_path)
        .unwrap();
    conceal(&mut app);
    let prepared = startup_ops::DirectMediaPrepared::image_fixture_for_test(new_path.clone());
    let (target, adoption) = app
        .install_direct_media_reader_for_open(prepared, ViewerOpenAdoption::ContextContinuation)
        .expect("excluded media must enter the direct reader");
    assert_eq!(
        target,
        crate::snapshot::SnapshotTarget::Fs(new_path.clone())
    );
    assert_eq!(adoption, ViewerOpenAdoption::ContextContinuation);
    assert_eq!(app.items.len(), 4);
    assert!(!app.conceal_mode);
    assert!(app.conceal_spread_ctx.is_none());
    assert_saved(&app, &old);
    assert_eq!(
        app.snapshot.as_ref().unwrap().saved_viewing_state.mode,
        SpreadMode::Ltr,
        "the return snapshot must capture the restored spread, not the edit's single view"
    );
    let new_idx = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(new_path.as_path()))
        .unwrap();
    let new_key = app.page_path_key(new_idx).unwrap();
    assert!(
        app.conceal_db
            .as_ref()
            .unwrap()
            .get_full_checked(&new_key)
            .unwrap()
            .is_none()
    );
    app.open_fullscreen(new_idx, HistoryTrigger::UserChosen);
    app.reconcile_page_edit_spread_pivots();
    assert_eq!(app.fullscreen_idx, Some(new_idx));
}

#[test]
fn page_edit_920_direct_reader_existing_target_keeps_erase_until_loaded_file_reopen_exits() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let path = app.items[1].drag_source_path().unwrap().to_path_buf();
    app.erase_mode = true;
    app.erase_mask_size = [5, 3];
    app.erase_mask = Some(vec![true; 15]);
    app.erase_spread_ctx = Some(pivot());
    let prepared = startup_ops::DirectMediaPrepared::image_fixture_for_test(path.clone());
    app.install_direct_media_reader_for_open(prepared, ViewerOpenAdoption::ContextContinuation)
        .expect("existing target needs no reader replacement");
    assert!(app.snapshot.is_none());
    assert!(app.erase_mode);
    assert!(app.erase_spread_ctx.is_some());
    let opened = app
        .open_loaded_file_fullscreen_with_adoption(
            &path,
            "page_edit_920_reopen",
            ViewerOpenAdoption::ContextContinuation,
        )
        .unwrap();
    assert_eq!(opened, 1);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.spread_mode, SpreadMode::Ltr);
    assert!(!app.erase_mode);
    assert!(app.erase_spread_ctx.is_none());
    assert!(app.erase_mask.is_none());
}

#[cfg(windows)]
#[test]
fn page_edit_920_linked_sync_same_item_refresh_exits_erase_and_restores_spread() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.settings.detached_viewer_enabled = true;
    app.settings.detached_viewer_open_images_in_window = false;
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.detached_viewer_independent_active = false;
    app.last_viewer_sync_stamp = None;
    app.erase_mode = true;
    app.erase_mask_size = [5, 3];
    app.erase_mask = Some(vec![true; 15]);
    app.erase_spread_ctx = Some(pivot());
    app.sync_detached_viewer_to_selected(&ctx);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.spread_mode, SpreadMode::Ltr);
    assert!(
        app.last_viewer_sync_stamp.is_some(),
        "the real same-item sync must run"
    );
    assert!(!app.erase_mode);
    assert!(app.erase_spread_ctx.is_none());
    assert!(app.erase_mask.is_none());
}
