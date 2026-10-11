//! Context-owned rating membership, reader landing and Undo regressions (§1.371).
use super::tests::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use crate::keymap::{RatingEdit, RatingStepDirection};
use crate::rating_membership::RatingAdoptionIntent;

const ONLY_THREE: [bool; 6] = [false, false, false, true, false, false];

fn install(app: &mut App, items: Vec<GridItem>) {
    app.items = items;
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.image_metas = vec![None; app.items.len()];
    app.visible_indices = (0..app.items.len()).collect();
    app.rating_cache.clear();
    for idx in 0..app.items.len() {
        let key = app.rating_path_key(idx).unwrap();
        app.rating_db.as_ref().unwrap().set(&key, 3).unwrap();
        app.rating_cache.insert(idx, 3);
    }
    app.selected = Some(0);
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
}

fn images(names: &[&str]) -> AppTestEnv {
    let mut app = setup_app();
    let folder = app.tmp.path().join("pages");
    std::fs::create_dir_all(&folder).unwrap();
    let items = names
        .iter()
        .map(|name| {
            let path = folder.join(format!("{name}.png"));
            image::RgbaImage::from_pixel(4, 4, image::Rgba([90, 100, 110, 255]))
                .save(&path)
                .unwrap();
            GridItem::Image(path)
        })
        .collect();
    install(&mut app, items);
    app.current_folder = Some(folder);
    app.settings.rating_filter = ONLY_THREE;
    app
}

fn anchor(app: &App) -> Option<usize> {
    app.grid_click_selection_anchor
        .and_then(|anchor| anchor.index_for_generation(app.items_generation))
}

fn key(app: &App, idx: usize) -> String {
    app.rating_path_key(idx).unwrap()
}

fn press_rating_key(app: &mut App, key: egui::Key) {
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    app.keyboard_owner_for_pass(&ctx);
    assert!(app.handle_fullscreen_root_key_input(&ctx));
    let _ = ctx.end_pass();
}

#[test]
fn item_keys_f1_through_f6_save_reader_target_when_grid_selection_differs() {
    let _input_guard = crate::key_input::lock_test_input();
    for (pressed, stars) in [
        (egui::Key::F1, 1),
        (egui::Key::F2, 2),
        (egui::Key::F3, 3),
        (egui::Key::F4, 4),
        (egui::Key::F5, 5),
        (egui::Key::F6, 0),
    ] {
        let mut app = images(&["a", "b", "c"]);
        app.settings.rating_filter = [true; 6];
        app.selected = Some(2);
        app.fullscreen_idx = Some(1);
        let page = key(&app, 1);
        let selection = key(&app, 2);
        press_rating_key(&mut app, pressed);
        assert_eq!(app.rating_db.as_ref().unwrap().get(&page), stars);
        assert_eq!(app.rating_db.as_ref().unwrap().get(&selection), 3);
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!(app.selected, Some(2));
        assert_eq!(app.meta_undo.undo_len(), usize::from(stars != 3));
    }
}

#[test]
fn custom_step_key_excludes_reader_once_and_bound_noop_keeps_generation_and_history() {
    let _input_guard = crate::key_input::lock_test_input();
    let mut app = images(&["a", "b", "c"]);
    app.keymap = crate::keymap::Keymap::from_ini_str("[Rating]\nRatingItemStepUp = F13\n");
    app.fullscreen_idx = Some(1);
    app.selected = Some(0);
    let page = key(&app, 1);
    press_rating_key(&mut app, egui::Key::F13);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&page), 4);
    assert_eq!(app.fullscreen_idx, Some(2));
    assert_eq!(app.selected, Some(2));
    assert_eq!(app.rating_publication_count, 1);
    assert_eq!(app.meta_undo.undo_len(), 1);
    app.settings.rating_filter = [true; 6];
    assert!(app.set_rating(2, 5));
    let generation = app.rating_session_write_generation;
    let publications = app.rating_publication_count;
    let undo = app.meta_undo.undo_len();
    assert!(
        !app.apply_rating_edit_to_fullscreen_item(2, RatingEdit::Step(RatingStepDirection::Up))
    );
    assert_eq!(app.rating_session_write_generation, generation);
    assert_eq!(app.rating_publication_count, publications);
    assert_eq!(app.meta_undo.undo_len(), undo);
    assert_eq!(app.fullscreen_idx, Some(2));
}

#[test]
fn rating_next_previous_and_undo_redo_preserve_filter_and_origin() {
    let mut app = images(&["a", "b", "c"]);
    app.fullscreen_idx = Some(1);
    app.selected = Some(0);
    app.grid_click_selection_anchor = Some(GridClickSelectionAnchor::new(0, app.items_generation));
    let page = key(&app, 1);
    assert!(app.set_rating(1, 4));
    assert_eq!(app.visible_indices, vec![0, 2]);
    assert_eq!(app.fullscreen_idx, Some(2));
    assert_eq!(app.selected, Some(2));
    assert_eq!(anchor(&app), Some(2));
    assert_eq!(app.meta_undo.undo_len(), 1);
    app.apply_meta_undo();
    assert_eq!(app.rating_db.as_ref().unwrap().get(&page), 3);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.selected, Some(1));
    assert_eq!(anchor(&app), Some(1));
    app.apply_meta_redo();
    assert_eq!(app.fullscreen_idx, Some(2));
    assert_eq!(app.meta_undo.undo_len(), 1);
    assert_eq!(app.settings.rating_filter, ONLY_THREE);
    // The removed final page has no later survivor, so the previous page wins.
    assert!(app.set_rating(2, 4));
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.selected, Some(0));
    assert_eq!(app.meta_undo.undo_len(), 2);
}

#[test]
fn details_descending_reader_moves_to_next_ordinal_in_both_reading_directions() {
    for direction in [
        crate::settings::ReadingDirection::Ltr,
        crate::settings::ReadingDirection::Rtl,
    ] {
        let mut app = images(&["a", "b", "c"]);
        app.settings.grid_view_mode = crate::settings::GridViewMode::Details;
        app.settings.details_sort_key = crate::settings::DetailsSortKey::Name;
        app.settings.details_sort_ascending = false;
        app.rebuild_details_order();
        assert_eq!(app.current_reader_order(), &[2, 1, 0]);
        app.reading_direction = direction;
        app.fullscreen_idx = Some(1);
        app.selected = Some(2);
        app.grid_click_selection_anchor =
            Some(GridClickSelectionAnchor::new(2, app.items_generation));
        assert!(app.set_rating(1, 4));
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.selected, Some(0));
        assert_eq!(anchor(&app), Some(0));
        assert_eq!(app.current_reader_order(), &[2, 0]);
    }
}

#[test]
fn explicit_filter_change_with_only_new_entrants_moves_into_adopted_reader() {
    let mut app = images(&["old", "entrant"]);
    let entrant = key(&app, 1);
    app.rating_db.as_ref().unwrap().set(&entrant, 4).unwrap();
    app.rating_cache.insert(1, 4);
    app.visible_indices = vec![0];
    app.settings.rating_filter = [false, false, false, false, true, false];
    app.release_rating_preview_membership(RatingAdoptionIntent::Reconcile);
    assert_eq!(app.visible_indices, vec![1]);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.selected, Some(1));
    assert_eq!(anchor(&app), Some(1));
    assert_eq!(app.meta_undo.undo_len(), 0);
}

#[test]
fn last_reader_page_returns_to_current_grid_container_and_ctrl_z_reopens_page() {
    let _input_guard = crate::key_input::lock_test_input();
    let mut app = images(&["only"]);
    let folder = app.current_folder.as_ref().unwrap().join("child");
    std::fs::create_dir_all(&folder).unwrap();
    let mut items = app.items.clone();
    items.push(GridItem::Folder(folder));
    install(&mut app, items);
    let current_folder = app.current_folder.clone();
    assert!(app.set_rating(0, 4));
    assert_eq!(app.fullscreen_idx, None);
    assert_eq!(app.visible_indices, vec![1]);
    assert_eq!(app.selected, Some(1));
    assert_eq!(anchor(&app), Some(1));
    assert_eq!(app.current_folder, current_folder);
    assert_eq!(app.meta_undo.undo_len(), 1);
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        modifiers: egui::Modifiers::CTRL,
        events: vec![egui::Event::Key {
            key: egui::Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        }],
        ..Default::default()
    });
    app.keyboard_owner_for_pass(&ctx);
    app.handle_meta_undo_keys(&ctx);
    let _ = ctx.end_pass();
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.selected, Some(0));
    assert_eq!(app.viewer_presentation, ViewerPresentation::Fullscreen);
    app.apply_meta_redo();
    assert_eq!(app.fullscreen_idx, None);
    assert_eq!(app.current_folder, current_folder);
    assert_eq!(app.settings.rating_filter, ONLY_THREE);
}

#[test]
fn only_page_and_only_grid_row_clear_both_cursor_and_shift_anchor() {
    let mut app = images(&["only"]);
    app.grid_click_selection_anchor = Some(GridClickSelectionAnchor::new(0, app.items_generation));
    assert!(app.set_rating(0, 4));
    assert!(app.visible_indices.is_empty());
    assert_eq!(app.fullscreen_idx, None);
    assert_eq!(app.selected, None);
    assert_eq!(anchor(&app), None);
    assert_eq!(app.meta_undo.undo_len(), 1);
    app.apply_meta_undo();
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.selected, Some(0));
    assert_eq!(anchor(&app), Some(0));
    assert_eq!(app.settings.rating_filter, ONLY_THREE);
}

#[test]
fn undo_restores_rating_value_without_reopening_page_hidden_by_nonrating_condition() {
    let mut app = images(&["a", "b"]);
    let original = key(&app, 0);
    assert!(app.set_rating(0, 4));
    assert_eq!(app.fullscreen_idx, Some(1));
    app.search_filter = Some([1].into_iter().collect());
    app.apply_meta_undo();
    assert_eq!(app.rating_db.as_ref().unwrap().get(&original), 3);
    assert_eq!(app.visible_indices, vec![1]);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.selected, Some(1));
    assert_eq!(app.search_filter, Some([1].into_iter().collect()));
    assert_eq!(app.settings.rating_filter, ONLY_THREE);
    assert_eq!(app.meta_undo.redo_len(), 1);
}

#[test]
fn getter_overlay_does_not_consume_membership_publication_receipt() {
    let mut app = images(&["a", "b"]);
    let page = key(&app, 0);
    let meta = app.rating_meta_for_idx(0);
    app.write_user_rating_shared_deferred(&page, 4, meta.as_ref())
        .unwrap();
    assert_eq!(app.get_rating(0), 4);
    assert_eq!(app.rating_session_write_seen_generation, 0);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.rating_publication_count, 0);
    app.publish_current_context_rating_writes();
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.rating_publication_count, 1);
    assert_eq!(
        app.rating_session_write_seen_generation,
        app.rating_session_write_generation
    );
    app.publish_current_context_rating_writes();
    assert_eq!(app.rating_publication_count, 1);
}

#[test]
fn rating_origin_survives_two_edits_and_items_reorder() {
    let mut app = images(&["a", "b", "c"]);
    let a = key(&app, 0);
    let b = key(&app, 1);
    assert!(app.set_rating(0, 4));
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.set_rating(1, 4));
    assert_eq!(app.fullscreen_idx, Some(2));
    assert_eq!(app.meta_undo.undo_len(), 2);
    let old = app.capture_rating_membership();
    app.items.swap(0, 2);
    app.bump_items_generation();
    app.rating_cache = [(0, 3), (1, 4), (2, 4)].into_iter().collect();
    app.visible_indices = vec![0];
    app.adopt_rating_membership(old, RatingAdoptionIntent::Reconcile);
    assert_eq!(app.fullscreen_idx, Some(0));
    app.apply_meta_undo();
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(key(&app, app.fullscreen_idx.unwrap()), b);
    app.apply_meta_undo();
    assert_eq!(app.fullscreen_idx, Some(2));
    assert_eq!(key(&app, app.fullscreen_idx.unwrap()), a);
    assert_eq!(anchor(&app), Some(2));
    assert_eq!(app.meta_undo.redo_len(), 2);
    assert_eq!(app.settings.rating_filter, ONLY_THREE);
}

#[test]
fn identity_keep_remaps_distinct_shift_anchor_without_navigation_in_both_grid_modes() {
    for mode in [
        crate::settings::GridViewMode::Thumbnail,
        crate::settings::GridViewMode::Details,
    ] {
        let mut app = images(&["a", "b", "c"]);
        app.settings.grid_view_mode = mode;
        app.details_order = vec![2, 1, 0];
        app.selected = Some(2);
        app.fullscreen_idx = Some(1);
        app.grid_click_selection_anchor =
            Some(GridClickSelectionAnchor::new(0, app.items_generation));
        let old = app.capture_rating_membership();
        app.items.swap(0, 2);
        app.bump_items_generation();
        app.visible_indices = vec![0, 1, 2];
        app.details_order = vec![0, 1, 2];
        app.adopt_rating_membership(old, RatingAdoptionIntent::Reconcile);
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!(anchor(&app), Some(2));
        assert_eq!(app.meta_undo.undo_len(), 0);
    }
}

#[test]
fn different_identity_at_same_raw_index_is_move_and_rebases_shift_anchor() {
    let mut app = images(&["a", "b", "c"]);
    app.grid_click_selection_anchor = Some(GridClickSelectionAnchor::new(2, app.items_generation));
    let replacement = app.items[1].clone();
    let old = app.capture_rating_membership();
    app.items = vec![replacement];
    app.bump_items_generation();
    app.visible_indices = vec![0];
    app.adopt_rating_membership(old, RatingAdoptionIntent::Reconcile);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.selected, Some(0));
    assert_eq!(anchor(&app), Some(0));
}

#[test]
fn zip_pdf_and_epub_item_operations_keep_page_and_container_keys_distinct() {
    for extension in ["zip", "pdf", "epub"] {
        let mut app = setup_app();
        let book = app.tmp.path().join(format!("book.{extension}"));
        let items = (0..2)
            .map(|page_num| {
                if extension == "zip" {
                    GridItem::ZipImage {
                        zip_path: book.clone(),
                        entry_name: format!("nested/{page_num}.png"),
                    }
                } else {
                    GridItem::PdfPage {
                        pdf_path: book.clone(),
                        page_num,
                        content_type: None,
                    }
                }
            })
            .collect();
        install(&mut app, items);
        app.current_folder = Some(book);
        app.settings.rating_filter = [true; 6];
        app.selected = Some(1);
        let page = key(&app, 0);
        let sibling = key(&app, 1);
        let container = app.current_container_rating_key_and_source().unwrap().0;
        assert_ne!(page, sibling);
        assert_ne!(page, container);
        assert!(app.apply_rating_edit_to_fullscreen_item(0, RatingEdit::Assign(4)));
        assert_eq!(app.rating_db.as_ref().unwrap().get(&page), 4);
        assert_eq!(app.rating_db.as_ref().unwrap().get(&sibling), 3);
        assert_eq!(app.rating_db.as_ref().unwrap().get(&container), 0);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(app.apply_rating_edit_to_current_container(RatingEdit::Assign(5)));
        assert_eq!(app.rating_db.as_ref().unwrap().get(&container), 5);
        assert_eq!(app.rating_db.as_ref().unwrap().get(&page), 4);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.selected, Some(1));
        assert_eq!(app.rating_publication_count, 2);
    }
}

#[test]
fn collection_root_and_detached_physical_keep_reader_membership_under_global_filter() {
    for scope in [
        ViewerNavigationScope::CollectionRoot,
        ViewerNavigationScope::DetachedPhysical,
    ] {
        let mut app = images(&["a", "b", "c"]);
        app.navigation_scope = scope;
        app.selected = Some(2);
        app.fullscreen_idx = Some(1);
        let order = app.current_reader_order().to_vec();
        assert!(app.set_rating(1, 4));
        assert_eq!(app.rating_cache.get(&1), Some(&4));
        assert_eq!(app.current_reader_order(), order);
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!(app.selected, Some(2));
        assert_eq!(app.settings.rating_filter, ONLY_THREE);
    }
}

#[cfg(windows)]
#[test]
fn undo_from_sibling_context_restores_origin_reader_without_moving_sibling() {
    let mut app = images(&["a", "b"]);
    let main = app.projected_viewer_context_id();
    assert!(app.set_rating(0, 4));
    assert_eq!(app.fullscreen_idx, Some(1));
    let items = app.items.clone();
    let rating_cache = app.rating_cache.clone();
    let folder = app.current_folder.clone();
    let sibling = app.build_window_context_for_test(1381, |owner| {
        owner.items = items;
        owner.rating_cache = rating_cache;
        owner.thumbnails = vec![ThumbnailState::Pending; 2];
        owner.image_metas = vec![None; 2];
        owner.visible_indices = vec![0, 1];
        owner.current_folder = folder;
        owner.navigation_scope = ViewerNavigationScope::DetachedPhysical;
        owner.fullscreen_idx = Some(1);
        owner.selected = Some(1);
        owner.viewer_presentation = ViewerPresentation::Fullscreen;
    });
    app.with_window_context_for_test(sibling, |owner| {
        assert_eq!(owner.projected_viewer_context_id(), sibling);
        owner.apply_meta_undo();
        assert_eq!(owner.projected_viewer_context_id(), sibling);
        assert_eq!(owner.fullscreen_idx, Some(1));
        assert_eq!(owner.selected, Some(1));
        assert_eq!(owner.visible_indices, vec![0, 1]);
    });
    assert_eq!(app.projected_viewer_context_id(), main);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.selected, Some(0));
    assert_eq!(app.rating_cache.get(&0), Some(&3));
    assert_eq!(app.meta_undo.redo_len(), 1);
    assert_eq!(app.settings.rating_filter, ONLY_THREE);
}

fn install_restore_request(
    app: &mut App,
) -> std::sync::mpsc::Sender<Result<crate::rating_view::RatingViewBuildResult, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.rating_view_pending = Some(crate::rating_view::RatingViewPending {
        stars: 3,
        sequence: 1,
        source_generation: app.items_generation,
        context: app.projected_viewer_context_id(),
        rating_write_generation: app.rating_session_write_generation,
        rating_preview: Default::default(),
        rating_adoption: RatingAdoptionIntent::RestoreOrigin(app.capture_rating_origin().unwrap()),
        sort: Default::default(),
        membership_only: true,
        navigation: None,
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        rx,
    });
    tx
}

#[test]
fn manual_open_close_and_redo_cancel_pending_restore_intent() {
    let mut app = images(&["a", "b"]);
    install_restore_request(&mut app);
    app.open_fullscreen(1, HistoryTrigger::UserChosen);
    assert!(
        app.rating_view_pending
            .as_ref()
            .is_none_or(|pending| pending.rating_adoption == RatingAdoptionIntent::Reconcile)
    );
    install_restore_request(&mut app);
    app.close_fullscreen();
    assert!(
        app.rating_view_pending
            .as_ref()
            .is_none_or(|pending| pending.rating_adoption == RatingAdoptionIntent::Reconcile)
    );
    install_restore_request(&mut app);
    app.apply_meta_redo();
    assert!(
        app.rating_view_pending
            .as_ref()
            .is_none_or(|pending| pending.rating_adoption == RatingAdoptionIntent::Reconcile)
    );
}

#[test]
fn combined_rating_and_next_key_rebases_navigation_to_the_new_reader_page() {
    let _input_guard = crate::key_input::lock_test_input();
    let mut app = images(&["a", "b", "c", "d"]);
    let original = key(&app, 0);
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        events: [egui::Key::F4, egui::Key::ArrowDown]
            .into_iter()
            .map(|key| egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            })
            .collect(),
        ..Default::default()
    });
    app.keyboard_owner_for_pass(&ctx);
    assert!(app.handle_fullscreen_root_key_input(&ctx));
    let _ = ctx.end_pass();
    assert_eq!(app.rating_db.as_ref().unwrap().get(&original), 4);
    assert_eq!(
        app.fullscreen_idx,
        Some(2),
        "Next must start after rating landed on B"
    );
    // The rating-induced landing retains its Undo, but this additional explicit Next
    // uses the released manual-navigation history boundary.
    assert_eq!(app.meta_undo.undo_len(), 0);
}

#[test]
fn explicit_reader_boundary_navigation_cancels_restore_intent() {
    let mut app = images(&["only"]);
    let _sender = install_restore_request(&mut app);
    app.handle_fs_navigation(
        &egui::Context::default(),
        false,
        false,
        None,
        None,
        None,
        crate::ui_fullscreen::FsPageNav::Boundary { at_end: true },
        None,
        0,
    );
    assert_eq!(
        app.rating_view_pending.as_ref().unwrap().rating_adoption,
        RatingAdoptionIntent::Reconcile
    );
    assert_eq!(app.fullscreen_idx, Some(0));
}

#[test]
#[cfg(windows)]
fn explicit_native_reader_boundary_navigation_cancels_restore_intent() {
    let mut app = setup_app();
    let item = GridItem::Video(app.tmp.path().join("only.mp4"));
    install(&mut app, vec![item]);
    let _sender = install_restore_request(&mut app);
    app.navigate_native_video_fullscreen(&egui::Context::default(), 0, 1);
    assert_eq!(
        app.rating_view_pending.as_ref().unwrap().rating_adoption,
        RatingAdoptionIntent::Reconcile
    );
    assert_eq!(app.fullscreen_idx, Some(0));
}

#[test]
fn last_media_rating_returns_to_grid_and_undo_restores_the_original_media_domain() {
    for video in [false, true] {
        let mut app = setup_app();
        let path = app
            .tmp
            .path()
            .join(if video { "only.mp4" } else { "only.wav" });
        install(
            &mut app,
            vec![if video {
                GridItem::Video(path)
            } else {
                GridItem::Audio(path)
            }],
        );
        app.settings.rating_filter = ONLY_THREE;
        let original = key(&app, 0);
        assert!(app.set_rating(0, 4));
        assert_eq!(app.fullscreen_idx, None);
        assert_eq!(app.meta_undo.undo_len(), 1);
        app.apply_meta_undo();
        assert_eq!(app.rating_db.as_ref().unwrap().get(&original), 3);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.viewer_presentation, ViewerPresentation::Fullscreen);
        assert_eq!(app.meta_undo.redo_len(), 1);
    }
}

#[test]
fn unchanged_page_rating_undo_keeps_presentation_panel_and_audio_mode() {
    let mut app = setup_app();
    let item = GridItem::Video(app.tmp.path().join("audio-mode.mp4"));
    install(&mut app, vec![item]);
    app.settings.rating_filter = [true; 6];
    app.viewer_presentation = ViewerPresentation::MainWindow;
    app.adjustment_mode = crate::ui_helpers::MetadataPanelOpenState::ByPointer;
    app.video_audio_mode = Some(0);
    assert!(app.set_rating(0, 4));
    app.apply_meta_undo();
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.viewer_presentation, ViewerPresentation::MainWindow);
    assert_eq!(
        app.adjustment_mode,
        crate::ui_helpers::MetadataPanelOpenState::ByPointer
    );
    assert_eq!(app.video_audio_mode, Some(0));
    assert_eq!(app.meta_undo.redo_len(), 1);
}

#[test]
fn explicit_filter_places_new_entrants_before_the_next_old_survivor() {
    let mut app = images(&["a", "b", "c", "d"]);
    app.settings.rating_filter = [false, false, false, true, false, true];
    for (idx, stars) in [(1, 4), (2, 4), (3, 5)] {
        let key = key(&app, idx);
        app.rating_db.as_ref().unwrap().set(&key, stars).unwrap();
        app.rating_cache.insert(idx, stars);
    }
    app.rebuild_visible_indices();
    assert_eq!(app.visible_indices, vec![0, 3]);
    app.settings.rating_filter = [false, false, false, false, true, true];
    app.rebuild_visible_indices();
    assert_eq!(app.visible_indices, vec![1, 2, 3]);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.selected, Some(1));
    assert_eq!(anchor(&app), Some(1));
}

#[test]
fn details_descending_rtl_filter_chooses_nearest_hidden_entrant_in_canonical_order() {
    for start in [2, 0] {
        let mut app = images(&["a", "b", "c", "d", "e"]);
        app.settings.grid_view_mode = crate::settings::GridViewMode::Details;
        app.settings.details_sort_key = crate::settings::DetailsSortKey::Name;
        app.settings.details_sort_ascending = false;
        app.reading_direction = crate::settings::ReadingDirection::Rtl;
        for idx in [0, 1, 3, 4] {
            let key = key(&app, idx);
            app.rating_db.as_ref().unwrap().set(&key, 4).unwrap();
            app.rating_cache.insert(idx, 4);
        }
        app.fullscreen_idx = Some(2);
        app.selected = Some(2);
        app.rebuild_visible_indices();
        if start == 0 {
            app.settings.rating_filter = [false, false, false, false, true, false];
            app.rebuild_visible_indices();
            app.fullscreen_idx = Some(0);
            app.selected = Some(0);
            let key = key(&app, 0);
            app.rating_db.as_ref().unwrap().set(&key, 3).unwrap();
            app.rating_cache.insert(0, 3);
            app.settings.rating_filter = ONLY_THREE;
            app.rebuild_visible_indices();
        }
        app.settings.rating_filter = [false, false, false, false, true, false];
        app.rebuild_visible_indices();
        assert_eq!(
            app.fullscreen_idx,
            Some(1),
            "start={start}, RTL must follow descending ordinal"
        );
        assert_eq!(app.selected, Some(1));
    }
}

#[test]
fn rating_items_replacement_clears_loaded_layers_before_reusing_the_same_slot() {
    let mut app = images(&["a", "b"]);
    app.fullscreen_idx = None;
    app.items_are_rating_view = true;
    app.rating_view_stars = 3;
    let layers = local_adjust_core::LocalAdjustmentLayers::new(vec![
        local_adjust_core::LocalAdjustmentLayer::new(
            "old-page",
            local_adjust_core::LocalMask::Full,
            local_adjust_core::LocalEffect::None,
        ),
    ]);
    app.set_local_adjust_layers_for_idx_memory_only(0, layers);
    assert!(app.local_adjust_page_layers.contains_key(&0));
    let replacement = app.items[1].clone();
    app.apply_rating_membership_view_result(crate::rating_view::RatingViewBuildResult {
        stars: 3,
        rows: vec![],
        skipped: 0,
        visibility_notice: None,
        prepared: Some(crate::rating_view::RatingViewPreparedItems {
            items: vec![replacement.clone()],
            image_metas: vec![None],
            existing_keys: Default::default(),
            pin_map: Default::default(),
            video_items: vec![],
            video_thumb_overrides: Default::default(),
            page_edits: crate::app::page_edit_snapshot::StablePageEditProjection {
                snapshot: Default::default(),
                projection: Default::default(),
                stamp: crate::page_edit_write_epoch::PAGE_EDIT_WRITES.sample(),
            },
            rating_stamp: crate::rating_db::RATING_WRITES.sample(),
            tag_stamp: crate::tags_db::TAG_WRITES.sample(),
            rating_cache: [(0, 3)].into_iter().collect(),
            tags_cache: Default::default(),
        }),
    });
    assert_eq!(app.items[0], replacement);
    assert!(
        !app.local_adjust_page_layers.contains_key(&0),
        "A's loaded layers leaked onto B's reused slot"
    );
    assert!(!app.local_adjust_pages.contains(&0));
}

#[test]
fn rating_worker_ordinary_supersession_preserves_the_live_restore_intent() {
    for stale_sequence in [false, true] {
        let mut app = images(&["a", "b"]);
        app.items_are_rating_view = true;
        app.rating_view_stars = 3;
        app.rating_view_sort = Default::default();
        app.rating_view_request_sequence = 1;
        let sender = install_restore_request(&mut app);
        let expected = app
            .rating_view_pending
            .as_ref()
            .unwrap()
            .rating_adoption
            .clone();
        if stale_sequence {
            app.rating_view_request_sequence += 1;
        } else {
            sender
                .send(Err("rating read changed during rating prepare".into()))
                .unwrap();
        }
        app.poll_rating_view();
        assert_eq!(
            app.rating_view_pending.as_ref().unwrap().rating_adoption,
            expected
        );
    }
}

fn seed_mixed_edit_and_rating_history(app: &mut App) {
    app.settings.rating_filter = [true; 6];
    let mut params = crate::adjustment::AdjustParams::default();
    params.brightness = 10.0;
    app.capture_adjustment_undo(
        crate::undo_stack::AdjustUndoScope::Page(0),
        None,
        Some(params),
        "discarded page edit".into(),
    );
    let non_rating = app.meta_undo.peek_undo().unwrap().clone();
    assert!(app.set_rating(0, 4));
    assert!(app.set_rating(1, 5));
    app.apply_meta_undo();
    app.meta_undo.push_redo(non_rating);
    assert_eq!(app.meta_undo.undo_len(), 2);
    assert_eq!(app.meta_undo.redo_len(), 2);
}

fn assert_only_rating_history(app: &App) {
    assert_eq!(app.meta_undo.undo_len(), 1);
    assert_eq!(app.meta_undo.redo_len(), 1);
    assert!(matches!(
        app.meta_undo.peek_undo(),
        Some(crate::undo_stack::UndoEntry::Rating { .. })
    ));
    assert!(matches!(
        app.meta_undo.peek_redo(),
        Some(crate::undo_stack::UndoEntry::Rating { .. })
    ));
}

fn prepared_rating_result(items: Vec<GridItem>) -> crate::rating_view::RatingViewBuildResult {
    let count = items.len();
    crate::rating_view::RatingViewBuildResult {
        stars: 3,
        rows: vec![],
        skipped: 0,
        visibility_notice: None,
        prepared: Some(crate::rating_view::RatingViewPreparedItems {
            items,
            image_metas: vec![None; count],
            existing_keys: Default::default(),
            pin_map: Default::default(),
            video_items: vec![],
            video_thumb_overrides: Default::default(),
            page_edits: crate::app::page_edit_snapshot::StablePageEditProjection {
                snapshot: Default::default(),
                projection: Default::default(),
                stamp: crate::page_edit_write_epoch::PAGE_EDIT_WRITES.sample(),
            },
            rating_stamp: crate::rating_db::RATING_WRITES.sample(),
            tag_stamp: crate::tags_db::TAG_WRITES.sample(),
            rating_cache: (0..count).map(|idx| (idx, 3)).collect(),
            tags_cache: Default::default(),
        }),
    }
}

#[test]
fn rating_edit_boundary_commits_local_gesture_to_outgoing_key_and_preserves_rating_redo() {
    let mut app = images(&["a", "b"]);
    seed_mixed_edit_and_rating_history(&mut app);
    let outgoing = app.page_path_key(0).unwrap();
    let replacement_key = app.page_path_key(1).unwrap();
    let before = local_adjust_core::LocalAdjustmentLayers::new(vec![]);
    let after = local_adjust_core::LocalAdjustmentLayers::new(vec![
        local_adjust_core::LocalAdjustmentLayer::new(
            "outgoing gesture",
            local_adjust_core::LocalMask::Full,
            local_adjust_core::LocalEffect::None,
        ),
    ]);
    app.local_adjust_mode = true;
    app.local_adjust_canvas_drag = Some(LocalAdjustCanvasDrag {
        fs_idx: 0,
        layer_idx: 0,
        kind: LocalAdjustCanvasDragKind::LinearGradientStart,
        start: [0.0, 0.0],
    });
    app.local_adjust_canvas_drag_before_layers = Some(before);
    app.set_local_adjust_layers_for_idx_memory_only(0, after.clone());

    app.finish_rating_membership_edit_boundary();
    assert!(app.local_adjust_canvas_drag.is_none());
    assert!(app.local_adjust_canvas_drag_before_layers.is_none());
    assert!(!app.local_adjust_mode);
    assert_only_rating_history(&app);

    // Retire the outgoing slot before the worker finishes: persistence must retain its key.
    app.items_are_rating_view = true;
    let replacement = app.items[1].clone();
    app.apply_rating_membership_view_result(prepared_rating_result(vec![replacement]));
    app.finish_local_adjust_writes();
    assert_eq!(
        app.local_adjust_db.as_ref().unwrap().get_layers(&outgoing),
        Some(after.as_ref().clone())
    );
    assert_eq!(
        app.local_adjust_db
            .as_ref()
            .unwrap()
            .get_layers(&replacement_key),
        None
    );
    assert_only_rating_history(&app);
    app.apply_meta_redo();
    assert_eq!(app.rating_db.as_ref().unwrap().get(&replacement_key), 5);
}

#[test]
fn rating_edit_boundary_commits_adjustment_drag_before_outgoing_slot_is_reused() {
    let mut app = images(&["a", "b"]);
    seed_mixed_edit_and_rating_history(&mut app);
    let outgoing = app.page_path_key(0).unwrap();
    let replacement_key = app.page_path_key(1).unwrap();
    let mut after = crate::adjustment::AdjustParams::default();
    after.brightness = 27.0;
    app.adjustment_page_params.insert(0, after.clone());
    app.adjustment_drag_session = Some(AdjustmentDragSession {
        fs_idx: 0,
        before: None,
    });
    app.adjustment_dragging = true;
    app.adjustment_mode = crate::ui_helpers::MetadataPanelOpenState::ByPointer;

    app.finish_rating_membership_edit_boundary();
    assert!(app.adjustment_drag_session.is_none());
    assert!(!app.adjustment_dragging);
    assert_eq!(
        app.adjustment_mode,
        crate::ui_helpers::MetadataPanelOpenState::Closed
    );
    assert_only_rating_history(&app);
    app.items_are_rating_view = true;
    let replacement = app.items[1].clone();
    app.apply_rating_membership_view_result(prepared_rating_result(vec![replacement]));
    assert_eq!(
        app.adjustment_db
            .as_ref()
            .unwrap()
            .get_page_params(&outgoing),
        Some(after)
    );
    assert_eq!(
        app.adjustment_db
            .as_ref()
            .unwrap()
            .get_page_params(&replacement_key),
        None
    );
    assert!(!app.adjustment_page_params.contains_key(&0));
    assert_only_rating_history(&app);
}

#[test]
fn rating_poll_rejects_prepared_slots_when_finishing_old_page_edit_changes_read_stamp() {
    let mut app = images(&["a", "b"]);
    app.items_are_rating_view = true;
    app.rating_view_stars = 3;
    app.rating_view_sort = Default::default();
    app.rating_view_request_sequence = 1;
    let outgoing = app.page_path_key(0).unwrap();
    let original_items = app.items.clone();
    let original_generation = app.items_generation;
    let sender = install_restore_request(&mut app);
    let expected_intent = app
        .rating_view_pending
        .as_ref()
        .unwrap()
        .rating_adoption
        .clone();
    let result = prepared_rating_result(vec![app.items[1].clone()]);
    let prepared_stamp = result.prepared.as_ref().unwrap().page_edits.stamp;
    let mut after = crate::adjustment::AdjustParams::default();
    after.brightness = 31.0;
    app.adjustment_page_params.insert(0, after.clone());
    app.adjustment_drag_session = Some(AdjustmentDragSession {
        fs_idx: 0,
        before: None,
    });
    assert!(crate::page_edit_write_epoch::PAGE_EDIT_WRITES.accepts(prepared_stamp));
    sender.send(Ok(result)).unwrap();

    app.poll_rating_view();

    assert!(app.adjustment_drag_session.is_none());
    assert_eq!(
        app.adjustment_db
            .as_ref()
            .unwrap()
            .get_page_params(&outgoing),
        Some(after)
    );
    assert!(!crate::page_edit_write_epoch::PAGE_EDIT_WRITES.accepts(prepared_stamp));
    assert_eq!(app.items, original_items);
    assert_eq!(app.items_generation, original_generation);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.selected, Some(0));
    let retry = app.rating_view_pending.as_ref().unwrap();
    assert_eq!(retry.rating_adoption, expected_intent);
    assert!(retry.sequence > 1);
}

#[test]
fn automatic_rating_open_and_close_finish_local_edit_without_losing_rating_redo() {
    for close in [false, true] {
        let mut app = images(&["a", "b"]);
        seed_mixed_edit_and_rating_history(&mut app);
        app.fullscreen_idx = Some(0);
        app.selected = Some(0);
        let outgoing = app.page_path_key(0).unwrap();
        let other = app.page_path_key(1).unwrap();
        let after = local_adjust_core::LocalAdjustmentLayers::new(vec![
            local_adjust_core::LocalAdjustmentLayer::new(
                "finish before rating landing",
                local_adjust_core::LocalMask::Full,
                local_adjust_core::LocalEffect::None,
            ),
        ]);
        app.local_adjust_mode = true;
        app.local_adjust_canvas_drag = Some(LocalAdjustCanvasDrag {
            fs_idx: 0,
            layer_idx: 0,
            kind: LocalAdjustCanvasDragKind::LinearGradientStart,
            start: [0.0, 0.0],
        });
        app.local_adjust_canvas_drag_before_layers =
            Some(local_adjust_core::LocalAdjustmentLayers::new(vec![]));
        app.set_local_adjust_layers_for_idx_memory_only(0, after.clone());
        if close {
            // An external membership change removes the remaining reader, preserving
            // the existing rating Redo rather than creating a new edit that clears it.
            app.write_user_rating_shared_deferred(&other, 4, None)
                .unwrap();
            app.rating_cache.insert(1, 4);
        }
        app.settings.rating_filter = ONLY_THREE;

        app.rebuild_visible_indices();

        assert_eq!(app.fullscreen_idx, if close { None } else { Some(1) });
        assert!(app.local_adjust_canvas_drag.is_none());
        assert!(!app.local_adjust_mode);
        assert_only_rating_history(&app);
        app.finish_local_adjust_writes();
        assert_eq!(
            app.local_adjust_db.as_ref().unwrap().get_layers(&outgoing),
            Some(after.as_ref().clone())
        );
        assert_eq!(
            app.local_adjust_db.as_ref().unwrap().get_layers(&other),
            None
        );
        assert_only_rating_history(&app);
        app.apply_meta_redo();
        assert_eq!(app.rating_db.as_ref().unwrap().get(&other), 5);
    }
}

#[test]
fn held_old_rating_ready_after_undo_cannot_remove_the_restored_reader() {
    let mut app = images(&["a", "b"]);
    app.items_are_rating_view = true;
    app.rating_view_stars = 3;
    app.rating_view_sort = Default::default();
    let original_items = app.items.clone();
    assert!(app.set_rating(0, 4));
    assert_eq!(app.fullscreen_idx, Some(1));
    let old_result = prepared_rating_result(vec![app.items[1].clone()]);
    let (sender, rx) = std::sync::mpsc::channel();
    let pending = app.rating_view_pending.as_mut().unwrap();
    let old_cancel = pending.cancel.clone();
    let old_read_generation = pending.rating_write_generation;
    pending.rx = rx;
    // The old worker has completed A-excluded facts, but adoption is held in its channel.
    sender.send(Ok(old_result)).unwrap();
    app.apply_meta_undo();
    assert!(app.rating_session_write_generation > old_read_generation);
    assert_eq!(app.fullscreen_idx, Some(0));
    let restored_items_generation = app.items_generation;
    assert_eq!(
        app.rating_view_pending
            .as_ref()
            .unwrap()
            .rating_write_generation,
        old_read_generation,
    );
    assert!(!old_cancel.load(std::sync::atomic::Ordering::Relaxed));

    // Release polling only after Undo: neither a superseded request nor its facts may adopt.
    app.poll_rating_view();
    assert!(old_cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(app.items, original_items);
    assert_eq!(app.items_generation, restored_items_generation);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.selected, Some(0));
    assert_eq!(anchor(&app), Some(0));
    assert_eq!(app.rating_cache.get(&0), Some(&3));
    assert_eq!(app.settings.rating_filter, ONLY_THREE);
    assert_eq!(app.meta_undo.redo_len(), 1);
    assert_eq!(
        app.rating_view_pending
            .as_ref()
            .unwrap()
            .rating_write_generation,
        app.rating_session_write_generation,
    );
}

fn controlled_rating_request(
    app: &mut App,
    intent: RatingAdoptionIntent,
) -> std::sync::mpsc::Sender<Result<crate::rating_view::RatingViewBuildResult, String>> {
    let membership_only = app
        .rating_view_pending
        .as_ref()
        .map(|pending| pending.membership_only)
        .unwrap_or(true);
    if let Some(pending) = app.rating_view_pending.take() {
        pending.cancel();
    }
    app.rating_view_request_sequence = app.rating_view_request_sequence.wrapping_add(1);
    let (sender, rx) = std::sync::mpsc::channel();
    app.rating_view_pending = Some(crate::rating_view::RatingViewPending {
        stars: app.rating_view_stars,
        sequence: app.rating_view_request_sequence,
        source_generation: app.items_generation,
        context: app.projected_viewer_context_id(),
        rating_write_generation: app.rating_session_write_generation,
        rating_preview: app.rating_preview_recipe(),
        rating_adoption: intent,
        sort: app.rating_view_sort,
        membership_only,
        navigation: None,
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        rx,
    });
    sender
}

fn ready_with_current_rating_facts(
    app: &App,
    items: Vec<GridItem>,
) -> crate::rating_view::RatingViewBuildResult {
    let mut ready = prepared_rating_result(items.clone());
    ready.rows = items
        .iter()
        .map(|item| {
            let (GridItem::Image(path) | GridItem::Folder(path)) = item else {
                panic!("controlled Rating fixture requires physical path keys");
            };
            crate::rating_view::RatingViewRow {
                key: crate::adjustment_db::normalize_path(path),
                item: item.clone(),
                image_meta: None,
                rated_at_ms: None,
            }
        })
        .collect();
    let prepared = ready.prepared.as_mut().unwrap();
    prepared.existing_keys = ready.rows.iter().map(|row| row.key.clone()).collect();
    prepared.rating_cache = ready
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| (index, app.rating_db.as_ref().unwrap().get(&row.key)))
        .collect();
    ready
}

fn reader_key(app: &App) -> Option<String> {
    app.fullscreen_idx
        .and_then(|index| app.rating_path_key(index))
}

fn rating_view_with_absent_undo_origin() -> (AppTestEnv, Vec<GridItem>) {
    rating_view_with_absent_origin(false)
}

pub(super) fn rating_view_with_absent_grid_undo_origin() -> (AppTestEnv, Vec<GridItem>) {
    rating_view_with_absent_origin(true)
}

fn rating_view_with_absent_origin(grid: bool) -> (AppTestEnv, Vec<GridItem>) {
    let mut app = images(&["b", "a", "c", "d"]);
    app.items_are_rating_view = true;
    app.rating_view_stars = 3;
    app.rating_view_sort = Default::default();
    app.fullscreen_idx = if grid { None } else { Some(1) };
    app.selected = Some(1);
    let all = app.items.clone();
    assert!(app.set_rating(1, 4));
    let survivors = vec![all[0].clone(), all[2].clone(), all[3].clone()];
    let sender = controlled_rating_request(&mut app, RatingAdoptionIntent::Reconcile);
    sender
        .send(Ok(ready_with_current_rating_facts(&app, survivors)))
        .unwrap();
    app.poll_rating_view();
    assert!(app.rating_view_pending.is_none());
    // The accepted Rating Ready starts a separate real IndexOnly owner. Finish that
    // prerequisite before Undo/manual input; only the later Rating result is held.
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.fullscreen_idx, if grid { None } else { Some(1) });
    assert_eq!(app.items[1], all[2]);
    assert_eq!(app.meta_undo.undo_len(), 1);
    app.apply_meta_undo();
    assert_eq!(app.meta_undo.redo_len(), 1);
    assert!(matches!(
        app.rating_view_pending.as_ref().unwrap().rating_adoption,
        RatingAdoptionIntent::RestoreOrigin(_)
    ));
    (app, all)
}

#[test]
fn rating_grid_undo_returns_f2_item_and_redo_excludes_it_in_both_orders() {
    let _input_guard = crate::key_input::lock_test_input();
    for view in [
        crate::settings::GridViewMode::Thumbnail,
        crate::settings::GridViewMode::Details,
    ] {
        let mut app = images(&["a", "b", "c"]);
        app.fullscreen_idx = None;
        app.settings.grid_view_mode = view;
        let next = if view == crate::settings::GridViewMode::Details {
            app.settings.details_sort_key = crate::settings::DetailsSortKey::Name;
            app.settings.details_sort_ascending = false;
            app.details_order = vec![2, 1, 0];
            0
        } else {
            2
        };
        app.move_grid_cursor(Some(1), crate::app::GridCursorIntent::Automatic);
        press_grid_key(&mut app, egui::Key::F2, egui::Modifiers::NONE);
        assert_eq!(app.selected, Some(next));
        assert_eq!(anchor(&app), Some(next));
        press_grid_key(&mut app, egui::Key::Z, egui::Modifiers::CTRL);
        assert_eq!(app.selected, Some(1));
        assert_eq!(anchor(&app), Some(1));
        assert!(app.scroll_to_selected);
        assert_eq!(app.fullscreen_idx, None);
        press_grid_key(&mut app, egui::Key::Y, egui::Modifiers::CTRL);
        assert_eq!(app.selected, Some(next));
        assert_eq!(anchor(&app), Some(next));
        assert_eq!(app.fullscreen_idx, None);
    }
}

#[test]
fn rating_grid_undo_manual_key_choice_before_undo_or_redo_wins_even_at_edge() {
    let _input_guard = crate::key_input::lock_test_input();
    for view in [
        crate::settings::GridViewMode::Thumbnail,
        crate::settings::GridViewMode::Details,
    ] {
        for redo in [false, true] {
            for pressed in [egui::Key::Home, egui::Key::ArrowLeft, egui::Key::ArrowRight] {
                let mut app = images(&["a", "b", "c"]);
                app.fullscreen_idx = None;
                app.settings.grid_view_mode = view;
                app.settings.grid_cols = 1;
                app.settings.grid_cursor_wrap = false;
                app.move_grid_cursor(Some(1), crate::app::GridCursorIntent::Automatic);
                assert!(app.set_rating(1, 4));
                if redo {
                    app.apply_meta_undo();
                }
                press_grid_key(&mut app, pressed, egui::Modifiers::NONE);
                let manual = app.selected;
                let shift = anchor(&app);
                if redo {
                    app.apply_meta_redo();
                } else {
                    app.apply_meta_undo();
                }
                assert_eq!(app.selected, manual);
                assert_eq!(anchor(&app), shift);
                assert_eq!(app.fullscreen_idx, None);
                if !redo {
                    app.apply_meta_redo();
                    app.apply_meta_undo();
                    assert_eq!(
                        app.selected, manual,
                        "cancel survives the entry's round trip"
                    );
                }
            }
        }
    }
}

#[test]
fn rating_grid_undo_empty_grid_restores_cursor_without_opening_reader() {
    for view in [
        crate::settings::GridViewMode::Thumbnail,
        crate::settings::GridViewMode::Details,
    ] {
        let mut app = images(&["only"]);
        app.fullscreen_idx = None;
        app.settings.grid_view_mode = view;
        assert!(app.set_rating(0, 2));
        assert_eq!(app.selected, None);
        assert_eq!(anchor(&app), None);
        app.apply_meta_undo();
        assert_eq!(app.selected, Some(0));
        assert_eq!(anchor(&app), Some(0));
        assert_eq!(app.fullscreen_idx, None);
        app.apply_meta_redo();
        assert_eq!(app.selected, None);
        assert_eq!(anchor(&app), None);
        assert_eq!(app.fullscreen_idx, None);
    }
}

pub(super) fn rating_grid_history_manual_input_case(
    view: crate::settings::GridViewMode,
    redo: bool,
    input: impl FnOnce(&mut App),
) {
    let mut app = images(&["a", "b", "c"]);
    app.fullscreen_idx = None;
    app.settings.grid_view_mode = view;
    app.settings.grid_cols = 1;
    app.settings.grid_cursor_wrap = false;
    app.move_grid_cursor(Some(1), crate::app::GridCursorIntent::Automatic);
    assert!(app.set_rating(1, 4));
    if redo {
        app.apply_meta_undo();
    }
    input(&mut app);
    let manual = app.selected;
    let shift = anchor(&app);
    if redo {
        app.apply_meta_redo();
    } else {
        app.apply_meta_undo();
    }
    assert_eq!(app.selected, manual);
    assert_eq!(anchor(&app), shift);
    assert_eq!(app.fullscreen_idx, None);
}

#[test]
fn rating_grid_undo_hidden_origin_keeps_cursor_and_redo_visible_origin_rebases_it() {
    for view in [
        crate::settings::GridViewMode::Thumbnail,
        crate::settings::GridViewMode::Details,
    ] {
        let mut app = images(&["a", "b", "c"]);
        app.fullscreen_idx = None;
        app.settings.grid_view_mode = view;
        app.settings.rating_filter[4] = true;
        app.move_grid_cursor(Some(1), crate::app::GridCursorIntent::Automatic);
        assert!(app.set_rating(1, 4));
        // Settings projection (not a manual cursor move) hides the before-value.
        app.settings.rating_filter = [false, false, false, false, true, false];
        for idx in [0, 2] {
            let key = key(&app, idx);
            app.rating_db.as_ref().unwrap().set(&key, 4).unwrap();
            app.rating_cache.insert(idx, 4);
        }
        app.release_rating_preview_membership(RatingAdoptionIntent::Reconcile);
        app.apply_meta_undo();
        let manual = app.selected;
        let shift = anchor(&app);
        assert_ne!(manual, Some(1));
        assert!(!app.visible_indices.contains(&1));
        // A second Undo write cannot resurrect an item excluded by another condition.
        app.search_filter = Some([0, 2].into_iter().collect());
        app.apply_meta_redo();
        assert_eq!(app.selected, manual);
        assert_eq!(anchor(&app), shift);
        assert!(!app.visible_indices.contains(&1));
        app.apply_meta_undo();
        app.search_filter = None;
        app.apply_meta_redo();
        assert_eq!(
            app.selected,
            Some(1),
            "Redo's after-value returns the original row"
        );
        assert_eq!(anchor(&app), Some(1));
        assert_eq!(app.fullscreen_idx, None);
    }
}

#[test]
fn rating_grid_undo_held_ready_restores_reindexed_item_once_or_keeps_manual_choice() {
    let _input_guard = crate::key_input::lock_test_input();
    for view in [
        crate::settings::GridViewMode::Thumbnail,
        crate::settings::GridViewMode::Details,
    ] {
        for manual in [false, true] {
            let (mut app, all) = rating_view_with_absent_grid_undo_origin();
            app.settings.grid_view_mode = view;
            let intent = app
                .rating_view_pending
                .as_ref()
                .unwrap()
                .rating_adoption
                .clone();
            let sender = controlled_rating_request(&mut app, intent);
            sender
                .send(Ok(ready_with_current_rating_facts(&app, all.clone())))
                .unwrap();
            if manual {
                press_grid_key(&mut app, egui::Key::Home, egui::Modifiers::NONE);
            }
            let selected = app.selected.and_then(|idx| app.rating_path_key(idx));
            app.poll_rating_view();
            app.finish_page_identity_preparation_for_test();
            assert_eq!(app.fullscreen_idx, None);
            assert_eq!(app.items, all);
            if manual {
                assert_eq!(
                    app.selected.and_then(|idx| app.rating_path_key(idx)),
                    selected
                );
                assert_eq!(anchor(&app), app.selected);
            } else {
                assert_eq!(app.selected, Some(1));
                assert_eq!(anchor(&app), Some(1));
            }
            let sender = controlled_rating_request(&mut app, RatingAdoptionIntent::Reconcile);
            sender
                .send(Ok(ready_with_current_rating_facts(&app, all.clone())))
                .unwrap();
            app.poll_rating_view();
            app.finish_page_identity_preparation_for_test();
            assert_eq!(app.fullscreen_idx, None);
            assert_eq!(app.selected, if manual { Some(0) } else { Some(1) });
        }
    }
}

/// Hold the real Rating Undo's Ready after the last reader leaves and its slot
/// is removed. Shared by keyboard and pad handler regressions; no native input.
pub(crate) fn rating_undo_grid_input_case(
    view: crate::settings::GridViewMode,
    selected: Option<usize>,
    expected: Option<usize>,
    accepted: bool,
    input: impl FnOnce(&mut App),
) {
    let mut app = images(&["origin"]);
    let mut all = app.items.clone();
    if selected.is_some() {
        for name in ["b", "c", "d"] {
            let path = app.current_folder.as_ref().unwrap().join(name);
            std::fs::create_dir_all(&path).unwrap();
            all.push(GridItem::Folder(path));
        }
    }
    install(&mut app, all.clone());
    app.items_are_rating_view = true;
    app.rating_view_stars = 3;
    app.settings.grid_view_mode = view;
    app.settings.grid_cols = 1;
    app.settings.grid_cursor_wrap = false;
    app.last_cell_h = 10.0;
    app.last_viewport_h = 10.0;
    assert!(app.set_rating(0, 4));
    assert_eq!(app.fullscreen_idx, None, "last reader returns to grid");
    let sender = controlled_rating_request(&mut app, RatingAdoptionIntent::Reconcile);
    sender
        .send(Ok(ready_with_current_rating_facts(&app, all[1..].to_vec())))
        .unwrap();
    app.poll_rating_view();
    app.finish_page_identity_preparation_for_test();
    assert!(!app.items.contains(&all[0]), "origin slot is gone");
    app.move_grid_cursor(selected, crate::app::GridCursorIntent::Automatic);
    app.apply_meta_undo();
    let intent = app
        .rating_view_pending
        .as_ref()
        .unwrap()
        .rating_adoption
        .clone();
    assert!(matches!(intent, RatingAdoptionIntent::RestoreOrigin(_)));
    let sender = controlled_rating_request(&mut app, intent.clone());
    sender
        .send(Ok(ready_with_current_rating_facts(&app, all.clone())))
        .unwrap();

    input(&mut app);

    assert_eq!(app.selected, expected, "manual input result");
    assert_eq!(
        app.rating_view_pending.as_ref().unwrap().rating_adoption,
        if accepted {
            RatingAdoptionIntent::Reconcile
        } else {
            intent
        },
        "only accepted manual navigation cancels the live intent"
    );
    let manual_key = app.selected.and_then(|idx| app.rating_path_key(idx));
    let manual_anchor = anchor(&app).and_then(|idx| app.rating_path_key(idx));

    app.poll_rating_view();
    assert!(
        app.rating_view_pending.is_none(),
        "Ready still adopts the set"
    );
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.items, all);
    if accepted {
        assert_eq!(
            app.fullscreen_idx, None,
            "Ready must not reopen Undo origin"
        );
        if manual_key.is_some() {
            assert_eq!(
                app.selected.and_then(|idx| app.rating_path_key(idx)),
                manual_key
            );
            assert_eq!(
                anchor(&app).and_then(|idx| app.rating_path_key(idx)),
                manual_anchor
            );
        }
    } else {
        assert_eq!(
            app.fullscreen_idx,
            Some(0),
            "unaccepted input keeps Undo restore"
        );
        assert_eq!(app.selected, Some(0));
        assert_eq!(anchor(&app), Some(0));
    }
    assert_eq!(app.settings.rating_filter, ONLY_THREE);
    assert_eq!(app.meta_undo.redo_len(), 1);
}

#[test]
fn rating_undo_grid_keys_cancel_held_restore_on_moves_edges_and_empty_sets() {
    let _input_guard = crate::key_input::lock_test_input();
    use crate::settings::GridViewMode;
    use egui::Key::*;
    for view in [GridViewMode::Thumbnail, GridViewMode::Details] {
        for (pressed, start, target) in [
            (ArrowRight, 0, 1),
            (ArrowRight, 2, 2),
            (ArrowLeft, 1, 0),
            (ArrowLeft, 0, 0),
            (ArrowDown, 0, 1),
            (ArrowDown, 2, 2),
            (ArrowUp, 1, 0),
            (ArrowUp, 0, 0),
            (Home, 1, 0),
            (Home, 0, 0),
            (End, 1, 2),
            (End, 2, 2),
            (PageDown, 1, 2),
            (PageDown, 2, 2),
            (PageUp, 1, 0),
            (PageUp, 0, 0),
        ] {
            rating_undo_grid_input_case(view, Some(start), Some(target), true, |app| {
                press_grid_key(app, pressed, egui::Modifiers::NONE);
            });
        }
        for pressed in [
            ArrowRight, ArrowLeft, ArrowDown, ArrowUp, Home, End, PageDown, PageUp,
        ] {
            rating_undo_grid_input_case(view, None, None, true, |app| {
                press_grid_key(app, pressed, egui::Modifiers::NONE);
            });
        }
    }
}

pub(super) fn press_grid_key(app: &mut App, key: egui::Key, modifiers: egui::Modifiers) {
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        modifiers,
        events: vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }],
        ..Default::default()
    });
    app.keyboard_owner_for_pass(&ctx);
    let _ = app.handle_keyboard(&ctx);
    let _ = ctx.end_pass();
}

#[test]
fn rating_undo_grid_unadmitted_keys_and_automatic_cursor_moves_keep_held_restore() {
    let _input_guard = crate::key_input::lock_test_input();
    use crate::settings::GridViewMode;
    for view in [GridViewMode::Thumbnail, GridViewMode::Details] {
        for pressed in [
            egui::Key::Home,
            egui::Key::End,
            egui::Key::PageUp,
            egui::Key::PageDown,
        ] {
            rating_undo_grid_input_case(view, Some(1), Some(1), false, |app| {
                press_grid_key(app, pressed, egui::Modifiers::SHIFT);
            });
        }
        rating_undo_grid_input_case(view, Some(1), Some(1), false, |app| {
            app.search_has_focus = true;
            press_grid_key(app, egui::Key::ArrowRight, egui::Modifiers::NONE);
            app.search_has_focus = false;
        });
        rating_undo_grid_input_case(view, Some(1), Some(2), false, |app| {
            app.move_grid_cursor(Some(2), crate::app::GridCursorIntent::Automatic);
        });
        rating_undo_grid_input_case(view, Some(1), Some(2), true, |app| {
            press_grid_key(app, egui::Key::ArrowRight, egui::Modifiers::SHIFT);
        });
        rating_undo_grid_input_case(view, Some(1), Some(0), true, |app| {
            app.keymap = crate::keymap::Keymap::from_ini_str("[Grid]\nGridMoveFirst = F13\n");
            press_grid_key(app, egui::Key::F13, egui::Modifiers::NONE);
        });
    }
}

#[test]
fn rating_undo_manual_next_previous_seek_and_close_survive_held_ready_completion() {
    for action in ["next", "previous", "seek", "close"] {
        let (mut app, all) = rating_view_with_absent_undo_origin();
        let intent = app
            .rating_view_pending
            .as_ref()
            .unwrap()
            .rating_adoption
            .clone();
        let sender = controlled_rating_request(&mut app, intent);
        sender
            .send(Ok(ready_with_current_rating_facts(&app, all.clone())))
            .unwrap();
        if action == "close" {
            app.close_fullscreen();
        } else {
            let nav = match action {
                "next" => crate::ui_fullscreen::FsPageNav::Delta(1),
                "previous" => crate::ui_fullscreen::FsPageNav::Delta(-1),
                "seek" => crate::ui_fullscreen::FsPageNav::Target(0),
                _ => unreachable!(),
            };
            app.handle_fs_navigation(
                &egui::Context::default(),
                false,
                false,
                None,
                None,
                None,
                nav,
                None,
                1,
            );
        }
        let manual_page = reader_key(&app);
        let manual_selection = app.selected.and_then(|index| app.rating_path_key(index));
        assert_eq!(manual_page.is_none(), action == "close");
        assert_eq!(
            app.rating_view_pending.as_ref().unwrap().rating_adoption,
            RatingAdoptionIntent::Reconcile
        );

        app.poll_rating_view();

        assert!(
            app.rating_view_pending.is_none(),
            "{action}: Ready was not accepted"
        );
        app.finish_page_identity_preparation_for_test();
        assert_eq!(app.items, all, "{action}");
        assert_eq!(
            reader_key(&app),
            manual_page,
            "{action}: Ready replayed Undo origin"
        );
        assert_eq!(
            app.selected.and_then(|index| app.rating_path_key(index)),
            manual_selection,
            "{action}: Ready moved the manual cursor"
        );
        assert_eq!(app.settings.rating_filter, ONLY_THREE);
        // A later successful request cannot replay the already cancelled origin either.
        let sender = controlled_rating_request(&mut app, RatingAdoptionIntent::Reconcile);
        sender
            .send(Ok(ready_with_current_rating_facts(&app, all.clone())))
            .unwrap();
        app.poll_rating_view();
        assert!(app.rating_view_pending.is_none());
        app.finish_page_identity_preparation_for_test();
        assert_eq!(
            reader_key(&app),
            manual_page,
            "{action}: second Ready reopened origin"
        );
    }
}

#[test]
fn rating_undo_redo_rejects_held_restore_ready_then_accepts_latest_without_reopening_origin() {
    let (mut app, all) = rating_view_with_absent_undo_origin();
    let origin = match &all[1] {
        GridItem::Image(path) => crate::adjustment_db::normalize_path(path),
        _ => unreachable!(),
    };
    let intent = app
        .rating_view_pending
        .as_ref()
        .unwrap()
        .rating_adoption
        .clone();
    let sender = controlled_rating_request(&mut app, intent);
    let old_cancel = app.rating_view_pending.as_ref().unwrap().cancel.clone();
    sender
        .send(Ok(ready_with_current_rating_facts(&app, all.clone())))
        .unwrap();
    let before_page = reader_key(&app);
    let before_items = app.items.clone();
    let before_generation = app.items_generation;

    app.apply_meta_redo();
    assert_eq!(app.rating_db.as_ref().unwrap().get(&origin), 4);
    app.poll_rating_view();
    assert!(old_cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(app.items, before_items);
    assert_eq!(app.items_generation, before_generation);
    assert_eq!(reader_key(&app), before_page);
    assert_eq!(
        app.rating_view_pending.as_ref().unwrap().rating_adoption,
        RatingAdoptionIntent::Reconcile
    );

    let sender = controlled_rating_request(&mut app, RatingAdoptionIntent::Reconcile);
    sender
        .send(Ok(ready_with_current_rating_facts(
            &app,
            before_items.clone(),
        )))
        .unwrap();
    app.poll_rating_view();
    assert!(app.rating_view_pending.is_none());
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.items, before_items);
    assert_eq!(reader_key(&app), before_page);
    assert_eq!(app.meta_undo.undo_len(), 1);
    assert_eq!(app.meta_undo.redo_len(), 0);
    assert_eq!(app.settings.rating_filter, ONLY_THREE);
    assert_ne!(reader_key(&app), Some(origin));
}

#[test]
fn rating_ring_poll_adopts_new_candidates_while_preview_and_live_sort_remain_owned() {
    for preview in [false, true] {
        let mut app = images(&["a", "b", "c", "d"]);
        let all = app.items.clone();
        let d = key(&app, 3);
        app.rating_db.as_ref().unwrap().set(&d, 0).unwrap();
        app.items = vec![all[0].clone(), all[1].clone()];
        app.thumbnails.truncate(2);
        app.image_metas.truncate(2);
        app.rating_cache = [(0, 3), (1, 3)].into_iter().collect();
        app.visible_indices = vec![0, 1];
        app.items_are_rating_view = true;
        app.rating_view_stars = 3;
        app.rating_view_sort = Default::default();
        let a = key(&app, 0);
        let mut picker =
            app.build_ring_picker_state(crate::ring_shortcut::RingShortcutContext::ImageFullscreen);
        if preview {
            picker.item_rating = 0;
            picker
                .dirty_rows
                .push(crate::ring_shortcut::RingPickerRowId::ItemRating);
        }
        app.ring_picker = Some(picker.clone());
        if preview {
            app.preview_picker_item_rating(&picker, 0);
        }
        let generation = app.rating_session_write_generation;
        assert_eq!(
            app.fullscreen_page_number_label(0).as_deref(),
            Some("1 / 2")
        );
        let candidates = vec![all[0].clone(), all[1].clone(), all[2].clone()];
        let sender = controlled_rating_request(&mut app, RatingAdoptionIntent::Reconcile);
        sender
            .send(Ok(ready_with_current_rating_facts(
                &app,
                candidates.clone(),
            )))
            .unwrap();
        app.poll_rating_view();
        assert!(app.rating_view_pending.is_none());
        // Ready adoption and its page-identity preparation are distinct owners;
        // live sort starts only after the installed list can admit the next operation.
        app.finish_page_identity_preparation_for_test();
        assert_eq!(app.items, candidates);
        assert_eq!(app.visible_indices.len(), 3);
        assert_eq!(app.get_still_image_indices().as_slice(), &[0, 1, 2]);
        assert_eq!(reader_key(&app), Some(a.clone()));
        assert_eq!(
            app.fullscreen_page_number_label(0).as_deref(),
            Some("1 / 3")
        );
        assert!(app.ring_picker.is_some());
        assert_eq!(
            app.rating_preview_recipe().entries.len(),
            usize::from(preview)
        );
        assert_eq!(app.rating_cache[&0], if preview { 0 } else { 3 });
        assert_eq!(app.meta_undo.undo_len(), 0);
        assert_eq!(app.rating_session_write_generation, generation);

        // Live sorting is a normal accepted result while the operation's stable key stays A.
        app.set_rating_view_sort(crate::rating_view::RatingViewSort::Normal(
            crate::settings::SortOrder::FileNameDesc,
        ));
        let sorted = vec![all[2].clone(), all[1].clone(), all[0].clone()];
        let sender = controlled_rating_request(&mut app, RatingAdoptionIntent::Reconcile);
        sender
            .send(Ok(ready_with_current_rating_facts(&app, sorted.clone())))
            .unwrap();
        app.poll_rating_view();
        app.finish_page_identity_preparation_for_test();
        assert_eq!(app.items, sorted);
        assert_eq!(reader_key(&app), Some(a.clone()));
        assert_eq!(
            app.fullscreen_page_number_label(2).as_deref(),
            Some("3 / 3")
        );
        // Hold one more successful Ready under the old recipe before ending the picker.
        let old_recipe_sender =
            controlled_rating_request(&mut app, RatingAdoptionIntent::Reconcile);
        let old_recipe_cancel = app.rating_view_pending.as_ref().unwrap().cancel.clone();
        old_recipe_sender
            .send(Ok(ready_with_current_rating_facts(&app, sorted.clone())))
            .unwrap();
        app.commit_ring_picker(&egui::Context::default());
        assert!(app.ring_picker.is_none());
        assert!(old_recipe_cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(app.rating_session_write_generation, generation);
        assert_eq!(app.meta_undo.undo_len(), usize::from(preview));
        if preview {
            assert_ne!(reader_key(&app), Some(a.clone()));
            assert_eq!(app.visible_indices.len(), 2);
        } else {
            assert_eq!(reader_key(&app), Some(a.clone()));
            assert_eq!(app.visible_indices.len(), 3);
        }
        let released = if preview {
            vec![all[2].clone(), all[1].clone()]
        } else {
            sorted
        };
        let sender = controlled_rating_request(&mut app, RatingAdoptionIntent::Reconcile);
        sender
            .send(Ok(ready_with_current_rating_facts(&app, released)))
            .unwrap();
        let after_close_page = reader_key(&app);
        app.poll_rating_view();
        assert!(app.rating_view_pending.is_none());
        app.finish_page_identity_preparation_for_test();
        assert_eq!(reader_key(&app), after_close_page);
        assert_eq!(
            app.get_still_image_indices().len(),
            if preview { 2 } else { 3 }
        );
        if preview {
            app.apply_meta_undo();
            let intent = app
                .rating_view_pending
                .as_ref()
                .unwrap()
                .rating_adoption
                .clone();
            let sender = controlled_rating_request(&mut app, intent);
            sender
                .send(Ok(ready_with_current_rating_facts(
                    &app,
                    vec![all[2].clone(), all[1].clone(), all[0].clone()],
                )))
                .unwrap();
            app.poll_rating_view();
            app.finish_page_identity_preparation_for_test();
            assert_eq!(reader_key(&app), Some(a));
            assert_eq!(app.meta_undo.redo_len(), 1);
        }
    }
}

#[test]
#[cfg(windows)]
fn rating_ring_recipe_and_ready_tokens_stay_with_owner_across_sibling_publication() {
    let mut app = images(&["a", "b", "c"]);
    app.items_are_rating_view = true;
    app.rating_view_stars = 3;
    app.rating_view_sort = Default::default();
    let main = app.projected_viewer_context_id();
    let all = app.items.clone();
    let a = key(&app, 0);
    let mut picker =
        app.build_ring_picker_state(crate::ring_shortcut::RingShortcutContext::ImageFullscreen);
    picker.item_rating = 0;
    picker
        .dirty_rows
        .push(crate::ring_shortcut::RingPickerRowId::ItemRating);
    app.ring_picker = Some(picker.clone());
    app.preview_picker_item_rating(&picker, 0);
    let main_recipe = app.rating_preview_recipe();
    let sender = controlled_rating_request(&mut app, RatingAdoptionIntent::Reconcile);
    let main_cancel = app.rating_view_pending.as_ref().unwrap().cancel.clone();
    sender
        .send(Ok(ready_with_current_rating_facts(&app, all.clone())))
        .unwrap();
    let folder = app.current_folder.clone();
    let sibling_items = all.clone();
    let sibling = app.build_window_context_for_test(1391, move |owner| {
        owner.items = sibling_items;
        owner.thumbnails = vec![ThumbnailState::Pending; 3];
        owner.image_metas = vec![None; 3];
        owner.rating_cache = [(0, 0), (1, 3), (2, 3)].into_iter().collect();
        owner.visible_indices = vec![1, 2];
        owner.current_folder = folder;
        owner.items_are_rating_view = true;
        owner.rating_view_stars = 3;
        owner.rating_view_sort = Default::default();
        owner.fullscreen_idx = Some(1);
        owner.selected = Some(1);
        owner.viewer_presentation = ViewerPresentation::Fullscreen;
    });
    app.with_window_context_for_test(sibling, |owner| {
        assert!(owner.rating_preview_recipe().entries.is_empty());
        let sender = controlled_rating_request(owner, RatingAdoptionIntent::Reconcile);
        assert_eq!(owner.rating_view_pending.as_ref().unwrap().context, sibling);
        assert!(
            owner
                .rating_view_pending
                .as_ref()
                .unwrap()
                .rating_preview
                .entries
                .is_empty()
        );
        sender
            .send(Ok(ready_with_current_rating_facts(
                owner,
                vec![all[1].clone(), all[2].clone()],
            )))
            .unwrap();
        owner.poll_rating_view();
        // A sibling write must use an admitted list, not a deferred IndexOnly open.
        owner.finish_page_identity_preparation_for_test();
        assert_eq!(owner.items.len(), 2);
        assert_eq!(owner.fullscreen_idx, Some(0));
        assert!(owner.set_rating(0, 4));
        assert_eq!(owner.fullscreen_idx, Some(1));
        assert!(owner.rating_preview_recipe().entries.is_empty());
    });
    assert_eq!(app.projected_viewer_context_id(), main);
    assert_eq!(app.rating_preview_recipe(), main_recipe);
    assert_eq!(reader_key(&app), Some(a.clone()));
    assert_eq!(app.visible_indices, vec![0, 2]);
    let original_pending = app.rating_view_pending.as_ref().unwrap();
    assert_eq!(original_pending.context, main);
    assert_eq!(original_pending.rating_preview, main_recipe);
    assert!(std::sync::Arc::ptr_eq(
        &original_pending.cancel,
        &main_cancel
    ));
    assert!(!main_cancel.load(std::sync::atomic::Ordering::Relaxed));
    let before_items = app.items.clone();
    let before_generation = app.items_generation;
    let before_page = reader_key(&app);
    // Production polling rejects the original held request after B's newer write.
    app.poll_rating_view();
    assert!(main_cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(app.items, before_items);
    assert_eq!(app.items_generation, before_generation);
    assert_eq!(reader_key(&app), before_page);
    assert_eq!(app.rating_view_pending.as_ref().unwrap().context, main);
    assert_eq!(
        app.rating_view_pending.as_ref().unwrap().rating_preview,
        main_recipe
    );
    let sender = controlled_rating_request(&mut app, RatingAdoptionIntent::Reconcile);
    sender
        .send(Ok(ready_with_current_rating_facts(
            &app,
            vec![all[0].clone(), all[2].clone()],
        )))
        .unwrap();
    app.poll_rating_view();
    app.finish_page_identity_preparation_for_test();
    assert_eq!(reader_key(&app), Some(a));
    assert!(app.ring_picker.is_some());
    assert_eq!(
        app.meta_undo.undo_len(),
        1,
        "only sibling's saved rating is finalized yet"
    );
    app.commit_ring_picker(&egui::Context::default());
    assert!(app.ring_picker.is_none());
    assert_eq!(app.meta_undo.undo_len(), 2);
}

#[test]
fn rating_landing_page_number_and_seek_domain_follow_single_spread_and_empty_membership() {
    for spread in [
        crate::settings::SpreadMode::Single,
        crate::settings::SpreadMode::Ltr,
    ] {
        let mut app = images(&["a", "b", "c"]);
        app.spread_mode = spread;
        assert_eq!(app.get_still_image_indices().as_slice(), &[0, 1, 2]);
        assert_eq!(
            app.fullscreen_page_number_label(0).as_deref(),
            Some(if spread.is_spread() {
                "1, 2 / 3"
            } else {
                "1 / 3"
            })
        );
        assert!(app.set_rating(0, 4));
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!(app.get_still_image_indices().as_slice(), &[1, 2]);
        assert_eq!(
            app.fullscreen_page_number_label(1).as_deref(),
            Some(if spread.is_spread() {
                "1, 2 / 2"
            } else {
                "1 / 2"
            })
        );
        assert_eq!(app.fullscreen_page_number_label(0), None);
        assert!(app.set_rating(1, 4));
        assert_eq!(app.fullscreen_idx, Some(2));
        assert_eq!(
            app.fullscreen_page_number_label(2).as_deref(),
            Some("1 / 1")
        );
        assert!(app.set_rating(2, 4));
        assert_eq!(app.fullscreen_idx, None);
        assert!(app.get_still_image_indices().is_empty());
        assert_eq!(app.fullscreen_page_number_label(2), None);
    }
}

#[test]
fn rating_deferred_index_and_hydration_resume_keep_rating_redo_and_drop_edit_history() {
    for hydration in [false, true] {
        let mut app = images(&["a", "b"]);
        seed_mixed_edit_and_rating_history(&mut app);
        app.fullscreen_idx = Some(0);
        let thumb_rx = if hydration {
            let folder = app.current_folder.clone().unwrap();
            Some(app.activate_live_sidecar_restore_for_test(folder))
        } else {
            app.prepare_current_listing_page_identity_index(Default::default(), None);
            None
        };
        app.open_fullscreen_for_rating_membership(1);
        assert_eq!(app.sidecar_restore_rating_deferred_target(), Some(1));
        assert_eq!(
            app.fullscreen_idx,
            Some(0),
            "defer must not publish its slot"
        );
        assert_only_rating_history(&app);
        // Complete the real IndexOnly owner, or the existing hydration fixture's terminal.
        app.finish_page_identity_preparation_for_test();
        assert_eq!(app.fullscreen_idx, Some(1), "hydration={hydration}");
        assert_eq!(app.sidecar_restore_rating_deferred_target(), None);
        assert_only_rating_history(&app);
        drop(thumb_rx);
    }
}

#[test]
fn rating_deferred_endpoint_filter_keep_move_empty_precedes_index_completion() {
    for painted in [None, Some(2)] {
        for (filter, target) in [
            (ONLY_THREE, Some(0)),
            ([false, false, false, false, true, false], Some(1)),
            ([false; 6], None),
        ] {
            let mut app = images(&["a", "b", "c"]);
            for (idx, stars) in [(1, 4), (2, 5)] {
                let key = key(&app, idx);
                app.rating_db.as_ref().unwrap().set(&key, stars).unwrap();
                app.rating_cache.insert(idx, stars);
            }
            app.fullscreen_idx = painted;
            app.prepare_current_listing_page_identity_index(Default::default(), None);
            app.open_fullscreen_for_rating_membership(0);
            assert_eq!(app.sidecar_restore_rating_deferred_target(), Some(0));
            app.settings.rating_filter = filter;
            app.rebuild_visible_indices();
            assert_eq!(app.sidecar_restore_rating_deferred_target(), target);
            assert!(
                app.sidecar_restore_active(),
                "filter must retain the index worker"
            );
            assert_eq!(
                app.fullscreen_idx,
                if target.is_some() { painted } else { None }
            );
            app.finish_page_identity_preparation_for_test();
            assert_eq!(
                app.fullscreen_idx, target,
                "painted={painted:?}, filter={filter:?}"
            );
            assert_eq!(app.selected, target);
        }
    }
}

fn rating_undo_accepted_but_index_deferred() -> AppTestEnv {
    let mut app = images(&["a", "b"]);
    app.items_are_rating_view = true;
    app.rating_view_stars = 3;
    let all = app.items.clone();
    assert!(app.set_rating(0, 4));
    assert!(app.set_rating(1, 4));
    let sender = controlled_rating_request(&mut app, RatingAdoptionIntent::Reconcile);
    sender
        .send(Ok(ready_with_current_rating_facts(&app, vec![])))
        .unwrap();
    app.poll_rating_view();
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.fullscreen_idx, None);
    app.apply_meta_undo();
    let intent = app
        .rating_view_pending
        .as_ref()
        .unwrap()
        .rating_adoption
        .clone();
    let sender = controlled_rating_request(&mut app, intent);
    sender
        .send(Ok(ready_with_current_rating_facts(&app, all)))
        .unwrap();
    app.poll_rating_view();
    assert_eq!(app.fullscreen_idx, None);
    assert_eq!(app.sidecar_restore_rating_deferred_target(), Some(1));
    assert_eq!(app.meta_undo.redo_len(), 1);
    app
}

#[test]
fn rating_undo_deferred_close_redo_and_manual_replacement_cannot_reopen_old_origin() {
    for operation in ["close", "redo", "manual"] {
        let mut app = rating_undo_accepted_but_index_deferred();
        match operation {
            "close" => app.close_fullscreen(),
            "redo" => app.apply_meta_redo(),
            "manual" => {
                app.settings.rating_filter = [true; 6];
                app.rebuild_visible_indices();
                app.open_fullscreen(0, HistoryTrigger::UserChosen);
            }
            _ => unreachable!(),
        }
        assert_eq!(
            app.sidecar_restore_rating_deferred_target(),
            None,
            "{operation}"
        );
        assert!(
            app.sidecar_restore_active(),
            "cancel must retain the index worker"
        );
        app.finish_page_identity_preparation_for_test();
        assert_eq!(
            app.fullscreen_idx,
            if operation == "manual" { Some(0) } else { None },
            "{operation}"
        );
        if operation == "redo" {
            assert_eq!(app.meta_undo.redo_len(), 0);
            assert_eq!(app.rating_db.as_ref().unwrap().get(&key(&app, 1)), 4);
        }
    }
}

#[test]
fn rating_deferred_keep_rebinds_the_chosen_key_after_items_generation_exchange() {
    let mut app = images(&["a", "b"]);
    app.items_are_rating_view = true;
    app.rating_view_stars = 3;
    app.fullscreen_idx = None;
    app.prepare_current_listing_page_identity_index(Default::default(), None);
    app.open_fullscreen_for_rating_membership(1);
    let chosen = key(&app, 1);
    let snapshot = app.capture_rating_membership();
    let items = vec![app.items[1].clone(), app.items[0].clone()];
    let result = ready_with_current_rating_facts(&app, items);
    // Exercise the accepted installation boundary directly while the previous owner exists.
    // Poll normally serializes this behind IndexOnly completion.
    app.apply_rating_membership_view_result(result);
    app.rebuild_visible_indices_for_rating_adoption();
    app.adopt_rating_membership(snapshot, RatingAdoptionIntent::Reconcile);
    assert_eq!(app.sidecar_restore_rating_deferred_target(), Some(0));
    assert_eq!(app.fullscreen_idx, None);
    app.finish_page_identity_preparation_for_test();
    assert_eq!(reader_key(&app), Some(chosen));
}

#[test]
fn rating_deferred_cancel_preserves_manual_and_unmatched_preparation_owners() {
    let mut app = images(&["a", "b"]);
    app.fullscreen_idx = None;
    app.prepare_current_listing_page_identity_index(Default::default(), None);
    assert!(app.defer_sidecar_restore_fullscreen(
        1,
        HistoryTrigger::UserChosen,
        FsOpenMaterialization::NavigationTargetMaterializationOnly,
        FsPageLoadContract::Sequential,
    ));
    app.cancel_rating_membership_restore();
    assert!(app.sidecar_restore_deferred_fullscreen_wait_active());
    app.finish_page_identity_preparation_for_test();
    assert_eq!(
        app.fullscreen_idx,
        Some(1),
        "Manual continuation remains authoritative"
    );

    app.fullscreen_idx = None;
    app.prepare_current_listing_page_identity_index(Default::default(), None);
    app.open_fullscreen_for_rating_membership(0);
    let source = app.current_folder.clone();
    app.current_folder = Some(app.tmp.path().join("different-source"));
    app.cancel_rating_membership_restore();
    assert_eq!(app.sidecar_restore_rating_deferred_target(), None);
    app.current_folder = source;
    assert_eq!(
        app.sidecar_restore_rating_deferred_target(),
        Some(0),
        "another source must not consume the owner"
    );
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.fullscreen_idx, Some(0));
}

#[test]
fn rating_deferred_hydration_supersedes_auto_and_required_first_display_once() {
    for auto_first_display in [false, true] {
        let mut app = images(&["a", "b"]);
        seed_mixed_edit_and_rating_history(&mut app);
        app.fullscreen_idx = None;
        let old_target = match &app.items[0] {
            GridItem::Image(path) => crate::snapshot::SnapshotTarget::Fs(path.clone()),
            _ => unreachable!(),
        };
        let folder = app.current_folder.clone().unwrap();
        let _thumb_rx = app.activate_live_sidecar_restore_for_test(folder);
        app.set_live_sidecar_first_display_for_test(
            if auto_first_display {
                sidecar_restore::FirstDisplayAction::AutoFullscreenFolder
            } else {
                sidecar_restore::FirstDisplayAction::None
            },
            Some(MainListAfterHydration::RequiredFullscreen {
                target: old_target,
                history_trigger: HistoryTrigger::UserChosen,
            }),
        );
        app.open_fullscreen_for_rating_membership(1);
        assert_only_rating_history(&app);
        app.finish_page_identity_preparation_for_test();
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_only_rating_history(&app);
    }
}

#[test]
fn rating_deferred_zip_hydration_consumes_the_older_bundled_enumeration_reopen() {
    let mut app = images(&["a", "b"]);
    seed_mixed_edit_and_rating_history(&mut app);
    app.fullscreen_idx = None;
    let folder = app.current_folder.clone().unwrap();
    let _thumb_rx = app.activate_live_sidecar_restore_for_test(folder);
    app.set_live_sidecar_first_display_for_test(
        sidecar_restore::FirstDisplayAction::Zip {
            has_foreign_archives: false,
        },
        None,
    );
    app.fs_nav_after_pdf_enumerate = Some(DeferredFsReopen {
        history_trigger: HistoryTrigger::UserChosen,
        resume_slideshow: false,
        target: DeferredFsTarget::None,
        resume_to_last_page: false,
        from_explicit_open: true,
        preserve_after_password_prompt: false,
    });
    app.open_fullscreen_for_rating_membership(1);
    assert!(
        app.fs_nav_after_pdf_enumerate.is_none(),
        "acceptance consumes the old reopen"
    );
    app.finish_page_identity_preparation_for_test();
    assert!(app.fs_nav_after_pdf_enumerate.is_none());
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_only_rating_history(&app);
}

#[test]
fn rating_deferred_live_close_redo_and_empty_do_not_revive_superseded_first_display() {
    for auto_first_display in [false, true] {
        for operation in ["close", "redo", "empty"] {
            let mut app = images(&["a", "b"]);
            seed_mixed_edit_and_rating_history(&mut app);
            app.fullscreen_idx = None;
            let old_target = match &app.items[0] {
                GridItem::Image(path) => crate::snapshot::SnapshotTarget::Fs(path.clone()),
                _ => unreachable!(),
            };
            let folder = app.current_folder.clone().unwrap();
            let _thumb_rx = app.activate_live_sidecar_restore_for_test(folder);
            app.set_live_sidecar_first_display_for_test(
                if auto_first_display {
                    sidecar_restore::FirstDisplayAction::AutoFullscreenFolder
                } else {
                    sidecar_restore::FirstDisplayAction::None
                },
                Some(MainListAfterHydration::RequiredFullscreen {
                    target: old_target,
                    history_trigger: HistoryTrigger::UserChosen,
                }),
            );
            app.open_fullscreen_for_rating_membership(1);
            assert_eq!(app.sidecar_restore_rating_deferred_target(), Some(1));
            assert_only_rating_history(&app);
            match operation {
                "close" => app.close_fullscreen(),
                "redo" => app.apply_meta_redo(),
                "empty" => {
                    app.settings.rating_filter = [false; 6];
                    app.rebuild_visible_indices();
                }
                _ => unreachable!(),
            }
            assert_eq!(app.sidecar_restore_rating_deferred_target(), None);
            assert!(app.sidecar_restore_active());
            app.finish_page_identity_preparation_for_test();
            assert_eq!(
                app.fullscreen_idx, None,
                "auto={auto_first_display}, op={operation}"
            );
            match operation {
                "close" => {
                    assert_eq!(app.meta_undo.undo_len(), 0);
                    assert_eq!(app.meta_undo.redo_len(), 0);
                }
                "redo" => {
                    assert_eq!(app.meta_undo.undo_len(), 2);
                    assert_eq!(app.meta_undo.redo_len(), 0);
                    assert_eq!(app.rating_db.as_ref().unwrap().get(&key(&app, 1)), 5);
                }
                "empty" => assert_only_rating_history(&app),
                _ => unreachable!(),
            }
        }
    }
}

#[test]
fn rating_deferred_warm_pdf_verification_keeps_chosen_endpoint_after_close_or_redo() {
    for operation in ["keep", "close", "redo"] {
        let mut app = setup_app();
        let pdf = app.tmp.path().join(format!("warm-{operation}.pdf"));
        std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
        let file_meta = std::fs::metadata(&pdf).unwrap();
        let mtime = crate::ui_helpers::mtime_secs(&file_meta);
        let file_size = file_meta.len();
        let parent = pdf.parent().unwrap().to_path_buf();
        app.get_or_open_catalog(&parent)
            .unwrap()
            .set_pdf_meta(
                pdf.file_name().unwrap().to_str().unwrap(),
                mtime,
                file_size as i64,
                2,
                false,
            )
            .unwrap();
        install(
            &mut app,
            (0..2)
                .map(|page_num| GridItem::PdfPage {
                    pdf_path: pdf.clone(),
                    page_num,
                    content_type: None,
                })
                .collect(),
        );
        app.current_folder = Some(pdf.clone());
        // Capture genuine rating history only after the PDF page identities are installed.
        seed_mixed_edit_and_rating_history(&mut app);
        app.fullscreen_idx = None;
        let _thumb_rx = app.activate_live_sidecar_restore_for_test(pdf.clone());
        app.set_live_sidecar_first_display_for_test(
            sidecar_restore::FirstDisplayAction::WarmPdf,
            None,
        );
        app.pdf_placeholder_count = Some(2);
        app.pdf_enumerate_pending = Some((
            pdf.clone(),
            None,
            crate::pdf_loader::completed_enumerate_result_handle(
                &pdf,
                Ok(crate::pdf_loader::PdfEnumerateResult {
                    pages: (0..2)
                        .map(|page_num| crate::pdf_loader::PdfPageEntry {
                            page_num,
                            mtime,
                            file_size,
                        })
                        .collect(),
                    direction: None,
                    stamp: None,
                }),
            ),
            Box::new(OpenRequestOwner::Navigation),
            None,
            PdfOpenPhase::CommittedVerification {
                placeholder_count: 2,
            },
            crate::pdf_loader::LeasedEpubPath::try_new(pdf.clone()).unwrap(),
        ));
        app.fs_nav_after_pdf_enumerate = Some(DeferredFsReopen {
            history_trigger: HistoryTrigger::UserChosen,
            resume_slideshow: false,
            target: DeferredFsTarget::None,
            resume_to_last_page: false,
            from_explicit_open: true,
            preserve_after_password_prompt: false,
        });
        app.open_fullscreen_for_rating_membership(1);
        assert!(
            app.fs_nav_after_pdf_enumerate.is_none(),
            "WarmPdf acceptance supersedes its delayed reopen"
        );
        assert_only_rating_history(&app);
        // A Ready verification result is held behind the existing hydration owner.
        app.poll_pdf_enumerate();
        assert!(app.pdf_enumerate_pending.is_some());
        match operation {
            "keep" => {}
            "close" => app.close_fullscreen(),
            "redo" => app.apply_meta_redo(),
            _ => unreachable!(),
        }
        app.finish_page_identity_preparation_for_test();
        let expected_page = if operation == "keep" { Some(1) } else { None };
        let expected_history = match operation {
            "keep" => (1, 1),
            "close" => (0, 0),
            "redo" => (2, 0),
            _ => unreachable!(),
        };
        assert_eq!(
            app.fullscreen_idx, expected_page,
            "{operation} after hydration"
        );
        assert_eq!(
            (app.meta_undo.undo_len(), app.meta_undo.redo_len()),
            expected_history
        );
        assert!(
            app.pdf_enumerate_pending.is_some(),
            "WarmPdf maintenance remains owned"
        );
        let installed = app.items.clone();
        let generation = app.items_generation;
        // Release the held result through the actual CommittedVerification consumer.
        app.poll_pdf_enumerate();
        assert!(app.pdf_enumerate_pending.is_none());
        assert!(app.pdf_placeholder_count.is_none());
        assert_eq!(app.items, installed);
        assert_eq!(app.items_generation, generation);
        assert_eq!(
            app.fullscreen_idx, expected_page,
            "{operation} after verification"
        );
        assert_eq!(
            (app.meta_undo.undo_len(), app.meta_undo.redo_len()),
            expected_history
        );
        assert!(app.fs_nav_after_pdf_enumerate.is_none());
        if operation == "redo" {
            assert_eq!(app.rating_db.as_ref().unwrap().get(&key(&app, 1)), 5);
        }
    }
}

#[test]
fn rating_grid_explicit_scope_and_search_acceptance_cancel_return_but_auto_keeps_it() {
    use crate::settings::GridViewMode;
    for view in [GridViewMode::Thumbnail, GridViewMode::Details] {
        for action in [
            "scope",
            "search",
            "explicit-noop",
            "automatic",
            "automatic-search",
        ] {
            let mut app = images(&["a", "b", "c"]);
            app.fullscreen_idx = None;
            app.settings.grid_view_mode = view;
            app.move_grid_cursor(Some(1), GridCursorIntent::Automatic);
            assert!(app.set_rating(1, 4));
            let current = app.selected;
            match action {
                "scope" => app.retire_main_list_requests_for_surface_switch(),
                "search" => app.execute_search(&egui::Context::default()),
                "explicit-noop" => app.move_grid_cursor(current, GridCursorIntent::ExplicitKeep),
                "automatic" => app.move_grid_cursor(Some(2), GridCursorIntent::Automatic),
                "automatic-search" => {
                    app.reexecute_search_for_installed_list(&egui::Context::default())
                }
                _ => unreachable!(),
            }
            app.apply_meta_undo();
            assert_eq!(
                app.selected,
                Some(if matches!(action, "automatic" | "automatic-search") {
                    1
                } else {
                    2
                }),
                "{action} {view:?}"
            );
        }
    }
}
