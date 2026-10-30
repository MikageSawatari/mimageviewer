//! §1.473: immutable return payloads survive preparation, replacement and refresh.
use super::history_parent_1473::{CaseEnv, settle, setup};
use super::*;
use crate::app::top_level_grid_view::{
    TopLevelGridOrigin, TopLevelGridRestore, TopLevelGridSurface,
};
use std::path::PathBuf;

fn history_fixture(portable: bool, slot: Option<QuickFolderSlotId>) -> (CaseEnv, PathBuf) {
    let mut env = setup(portable);
    env.active_quick_folder_slot = slot;
    env.settings.auto_fullscreen_zip_pdf = false;
    env.settings.auto_fullscreen_image_folders = false;
    env.settings.detached_viewer_open_images_in_window = false;
    env.settings.sidecar_backup_enabled = false;
    env.settings.tag_sidecar_backup_enabled = false;
    let origin = env
        .settings
        .book_root
        .as_ref()
        .unwrap()
        .join("lifecycle-origin");
    std::fs::create_dir_all(&origin).unwrap();
    env.load_folder(origin.clone());
    settle(&mut env);
    env.enter_reading_history_from_menu();
    settle(&mut env);
    assert!(matches!(
        env.top_level_grid_view.surface(),
        TopLevelGridSurface::ReadingHistory(_)
    ));
    (env, origin)
}

fn root_origin(app: &App) -> TopLevelGridOrigin {
    let origin = app.capture_top_level_origin().expect("visible root origin");
    assert!(matches!(
        origin.restore,
        TopLevelGridRestore::ReadingHistory(_)
    ));
    origin
}

fn assert_cursors(app: &App, expected: &FolderNavHistorySnapshot) {
    assert_eq!(app.folder_nav_back_stack, expected.back_stack);
    assert_eq!(app.folder_nav_forward_stack, expected.forward_stack);
    for (actual, expected) in app
        .quick_folder_workspaces
        .iter()
        .zip(&expected.quick_folder_workspaces)
    {
        assert_eq!(actual.history, expected.history);
    }
    assert_eq!(
        app.active_quick_folder_slot,
        expected.active_quick_folder_slot
    );
}

#[test]
fn section1473_lifecycle_root_exit_prepares_without_commit_and_repeated_input_adopts_once() {
    for portable in [false, true] {
        for slot in [None, Some(QuickFolderSlotId::A), Some(QuickFolderSlotId::B)] {
            let (mut env, origin_path) = history_fixture(portable, slot);
            let source = root_origin(&env);
            let TopLevelGridRestore::ReadingHistory(root) = &source.restore else {
                unreachable!()
            };
            let ReadingHistoryExit::RecordedEntrance(entrance) = &root.exit else {
                panic!("explicit menu entrance must be recorded");
            };
            assert_eq!(
                entrance.restore,
                TopLevelGridRestore::Folder(origin_path.clone())
            );
            let before = env.folder_nav_history_snapshot();
            let (back, forward) = env.active_folder_history_vectors();
            let (plan, destination) = FolderHistoryPlan::reading_history_root_exit(
                env.folder_nav_current_entry().unwrap(),
                back,
                forward,
                slot,
                root,
            );
            let plan = plan.expect("the recorded entrance survives in this fixture");
            assert_eq!(&destination, entrance.as_ref());
            env.exit_reading_history_root();
            assert!(
                env.top_level_grid_view
                    .history_navigation_transition()
                    .is_some()
            );
            assert_eq!(root_origin(&env), source);
            assert_cursors(&env, &before);
            env.exit_reading_history_root();
            assert_eq!(root_origin(&env), source);
            assert_cursors(&env, &before);
            settle(&mut env);
            assert_eq!(env.effective_folder(), Some(origin_path));
            assert_eq!(env.facet_navigation.route(), &destination.route);
            let (back, forward) = env.active_folder_history_vectors();
            assert_eq!(back, plan.virtual_back);
            assert_eq!(forward, plan.virtual_forward);
            for untouched in [QuickFolderSlotId::A, QuickFolderSlotId::B] {
                if slot != Some(untouched) {
                    assert_eq!(
                        env.quick_folder_workspaces[untouched.index()].history,
                        before.quick_folder_workspaces[untouched.index()].history
                    );
                }
            }
        }
    }
}

#[test]
fn section1473_lifecycle_cancelled_root_exit_preserves_payload_and_can_be_reissued() {
    for portable in [false, true] {
        let (mut env, origin_path) = history_fixture(portable, None);
        let source = root_origin(&env);
        let before = env.folder_nav_history_snapshot();
        env.exit_reading_history_root();
        assert!(
            env.top_level_grid_view
                .history_navigation_transition()
                .is_some()
        );
        env.retire_main_list_requests_for_surface_switch();
        assert!(
            env.top_level_grid_view
                .history_navigation_transition()
                .is_none()
        );
        settle(&mut env);
        assert_eq!(root_origin(&env), source);
        assert_cursors(&env, &before);
        env.exit_reading_history_root();
        settle(&mut env);
        assert_eq!(env.effective_folder(), Some(origin_path));
    }
}

#[test]
fn section1473_lifecycle_late_root_exit_cannot_pop_a_switched_slot() {
    for portable in [false, true] {
        for source_slot in [None, Some(QuickFolderSlotId::A), Some(QuickFolderSlotId::B)] {
            let (mut env, origin_path) = history_fixture(portable, source_slot);
            let destination_slot = if source_slot == Some(QuickFolderSlotId::A) {
                QuickFolderSlotId::B
            } else {
                QuickFolderSlotId::A
            };
            let destination = origin_path.parent().unwrap().join("other-slot");
            std::fs::create_dir_all(&destination).unwrap();
            env.set_quick_folder_slot_target(destination_slot, destination.clone());
            env.active_quick_folder_slot = source_slot;
            env.quick_folder_workspaces[destination_slot.index()]
                .history
                .back_stack =
                vec![FolderNavHistoryTarget::Path(destination.join("back-marker")).into()];
            env.quick_folder_workspaces[destination_slot.index()]
                .history
                .forward_stack =
                vec![FolderNavHistoryTarget::Path(destination.join("forward-marker")).into()];
            let source = root_origin(&env);
            let before = env.folder_nav_history_snapshot();
            env.exit_reading_history_root();
            assert_eq!(root_origin(&env), source);
            assert_cursors(&env, &before);
            // Hold the actual request/worker, then deliver it after the real slot switch wins.
            let delayed = env
                .top_level_grid_view
                .take_history_navigation_transition()
                .unwrap();
            assert!(env.start_quick_folder_slot_switch(destination_slot, destination.clone()));
            settle(&mut env);
            assert_eq!(env.active_quick_folder_slot, Some(destination_slot));
            assert_eq!(env.effective_folder(), Some(destination.clone()));
            let adopted = env.folder_nav_history_snapshot();
            env.top_level_grid_view
                .set_history_navigation_transition(Some(delayed));
            settle(&mut env);
            assert!(
                env.top_level_grid_view
                    .history_navigation_transition()
                    .is_none()
            );
            assert_eq!(env.effective_folder(), Some(destination));
            assert_cursors(&env, &adopted);
        }
    }
}

#[test]
fn section1473_lifecycle_rating_refresh_membership_and_cancel_keep_history_origin() {
    for portable in [false, true] {
        let (mut env, origin_path) = history_fixture(portable, None);
        let history = root_origin(&env);
        env.enter_rating_view_from_menu(3);
        settle(&mut env);
        let expected_return = Some(Box::new(history.clone()));
        assert_eq!(env.rating_view_return_to, expected_return);
        let before = env.folder_nav_history_snapshot();
        let old_rating = env.folder_nav_current_target();
        env.set_rating_view_sort(crate::rating_view::RatingViewSort::Normal(
            SortOrder::FileNameDesc,
        ));
        assert_eq!(env.rating_view_return_to, expected_return);
        settle(&mut env);
        assert_eq!(env.rating_view_return_to, expected_return);
        assert_cursors(&env, &before);
        env.reload_current_rating_view_preserving_sort();
        assert_eq!(env.rating_view_return_to, expected_return);
        settle(&mut env);
        env.request_rating_view_membership_refresh();
        assert_eq!(env.rating_view_return_to, expected_return);
        settle(&mut env);
        assert_eq!(env.folder_nav_current_target(), old_rating);
        assert_eq!(env.rating_view_return_to, expected_return);
        assert_cursors(&env, &before);
        env.enter_rating_view_from_menu(5);
        assert!(env.rating_view_pending.is_some());
        assert_eq!(env.folder_nav_current_target(), old_rating);
        assert_eq!(env.rating_view_return_to, expected_return);
        assert_cursors(&env, &before);
        env.close_rating_view();
        settle(&mut env);
        assert!(env.rating_view_pending.is_none());
        assert_eq!(env.folder_nav_current_target(), old_rating);
        assert_eq!(env.rating_view_return_to, expected_return);
        assert_cursors(&env, &before);
        env.enter_rating_view_from_menu(5);
        settle(&mut env);
        assert_eq!(env.rating_view_stars, 5);
        assert_eq!(env.rating_view_return_to, expected_return);
        env.close_rating_view();
        settle(&mut env);
        assert_eq!(root_origin(&env), history);
        env.exit_reading_history_root();
        settle(&mut env);
        assert_eq!(env.effective_folder(), Some(origin_path));
    }
}

#[test]
fn section1473_lifecycle_rating_none_and_bookmark_origins_survive_history_roundtrip() {
    for portable in [false, true] {
        for bookmark_origin in [false, true] {
            let (mut env, _) = history_fixture(portable, None);
            if bookmark_origin {
                env.open_bookmark_browser();
                settle(&mut env);
            }
            let bookmark = bookmark_origin.then(|| env.capture_top_level_origin().unwrap());
            env.enter_rating_view_from_menu(3);
            settle(&mut env);
            if !bookmark_origin {
                // Explicitly model the released no-saved-origin Rating root.
                env.rating_view_return_to = None;
            }
            let rating = env.capture_top_level_origin().unwrap();
            env.enter_reading_history_from_menu();
            settle(&mut env);
            // An unrelated live slot must neither fill a captured None nor replace Bookmarks.
            env.rating_view_return_to = Some(crate::app::tests::grid_origin_for_test(
                TopLevelGridRestore::Folder(PathBuf::from("unrelated-rating-origin")),
            ));
            env.exit_reading_history_root();
            settle(&mut env);
            assert_eq!(env.capture_top_level_origin(), Some(rating));
            if let Some(bookmark) = bookmark {
                assert_eq!(env.rating_view_return_to.as_deref(), Some(&bookmark));
                env.close_rating_view();
                settle(&mut env);
                assert!(env.items_are_bookmark_view);
                assert_eq!(env.capture_top_level_origin(), Some(bookmark));
                assert!(matches!(
                    env.grid_parent_nav_target(),
                    Some(crate::ui_main::AddressBarNav::HistoryBack)
                ));
            } else {
                assert!(env.rating_view_return_to.is_none());
                let current_folder = env.current_folder.clone();
                let before = env.folder_nav_history_snapshot();
                env.close_rating_view();
                assert!(!env.items_are_rating_view);
                assert!(env.rating_view_return_to.is_none());
                assert_eq!(env.current_folder, current_folder);
                assert_cursors(&env, &before);
            }
        }
    }
}
