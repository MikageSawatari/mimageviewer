//! Revisit policy through accepted App loads; all data belongs to AppTestEnv.
use super::*;

fn fixture(restore: bool) -> (phase_c_support::AppTestEnv, PathBuf, PathBuf) {
    let mut env = setup_app_for_test();
    env.active_quick_folder_slot = None;
    env.settings.restore_folder_cursor_on_revisit = restore;
    env.settings.auto_fullscreen_image_folders = false;
    env.settings.auto_fullscreen_zip_pdf = false;
    let a = env.tmp.path().join("A");
    let b = env.tmp.path().join("B");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    for n in 0..35 {
        image::RgbImage::new(1, 1)
            .save(a.join(format!("{n:02}.png")))
            .unwrap();
    }
    open(&mut env, &a);
    env.selected = Some(29);
    env.grid_scroll.set_position(420.0);
    (env, a, b)
}

fn open(app: &mut App, path: &Path) {
    assert!(app.load_folder_with_scan_owned(
        path.to_path_buf(),
        None,
        OpenRequestOwner::Navigation,
        StartupListIntent::ExplicitList,
    ));
    app.finish_page_identity_preparation_for_test();
}

fn assert_position(app: &App, selected: Option<usize>, scroll: f32) {
    assert_eq!(app.selected, selected);
    assert_eq!(app.grid_scroll.offset(), scroll);
}

fn stack_entry_fixture(restore: bool) -> (phase_c_support::AppTestEnv, PathBuf) {
    let mut env = setup_app_for_test();
    env.active_quick_folder_slot = None;
    env.settings.restore_folder_cursor_on_revisit = restore;
    env.settings.auto_fullscreen_image_folders = false;
    env.settings.stack_script_enabled = false;
    env.settings.restore_last_cursor = true;
    env.settings.stack_separator = '_';
    env.settings.sort_order = crate::settings::SortOrder::FileName;
    env.settings.facet_filter.exts.insert("png".into());
    let folder = env.tmp.path().join("stack-entry");
    std::fs::create_dir_all(&folder).unwrap();
    for name in ["a_1.jpg", "a_2.jpg", "b_1.png", "c_1.png"] {
        image::RgbImage::new(1, 1).save(folder.join(name)).unwrap();
    }
    open(&mut env, &folder);
    env.stack_mode_requested = true;
    env.grid_scroll.set_position(420.0);
    // The same-path ExplicitList is the intent used by a favorite/address open.
    open(&mut env, &folder);
    assert!(env.stack_script_pending.is_some());
    assert!(
        env.stack_view.is_none(),
        "hold aggregate adoption until the test polls"
    );
    assert_eq!(env.selected, Some(2));
    assert_eq!(env.items[2].name(), "b_1.png");
    (env, folder)
}

fn finish_stack_entry(app: &mut App) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.stack_script_pending.is_some() || app.sidecar_restore_active() {
        assert!(
            std::time::Instant::now() < deadline,
            "stack entry did not settle"
        );
        app.poll_stack_script(&ctx);
        app.finish_page_identity_preparation_for_test();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(app.stack_view.is_some());
}

#[test]
fn section1795_off_stack_same_path_entry_selects_final_visible_first() {
    let (mut env, _) = stack_entry_fixture(false);
    finish_stack_entry(&mut env);
    assert_eq!(
        env.items
            .iter()
            .map(|item| item.name().into_owned())
            .collect::<Vec<_>>(),
        ["a", "b_1.png", "c_1.png"]
    );
    assert_eq!(env.current_grid_order(), [1, 2]);
    assert_position(&env, Some(1), 0.0);
    assert_eq!(env.items[env.selected.unwrap()].name(), "b_1.png");
}

#[test]
fn section1795_stack_pending_explicit_cursor_wins_over_entry_policy() {
    for restore in [false, true] {
        for keep in [false, true] {
            let (mut env, folder) = stack_entry_fixture(restore);
            // Keep a row after the chosen item so accidental index clamping cannot
            // make a raw-index restore look like an identity-preserving selection.
            image::RgbImage::new(1, 1)
                .save(folder.join("d_1.png"))
                .unwrap();
            open(&mut env, &folder);
            if keep {
                env.move_grid_cursor(Some(3), GridCursorIntent::Automatic);
                env.move_grid_cursor(Some(3), GridCursorIntent::ExplicitKeep);
            } else {
                env.move_grid_cursor(Some(3), GridCursorIntent::Explicit(GridCursorMove::Move));
            }
            finish_stack_entry(&mut env);
            assert_eq!(env.selected, Some(2), "restore={restore}, keep={keep}");
            assert_eq!(env.items[env.selected.unwrap()].name(), "c_1.png");
            assert!(env.grid_scroll.reveal_requested());
        }
    }
}

#[test]
fn section1795_off_stack_new_entry_retires_previous_pending_selection() {
    let (mut env, folder) = stack_entry_fixture(false);
    env.move_grid_cursor(Some(3), GridCursorIntent::Explicit(GridCursorMove::Move));
    open(&mut env, &folder);
    assert_eq!(env.selected, Some(2));
    finish_stack_entry(&mut env);
    assert_position(&env, Some(1), 0.0);
}

#[test]
fn section1795_off_stack_load_hints_survive_final_aggregation() {
    for hint in ["name", "operation", "startup"] {
        let (mut env, folder) = stack_entry_fixture(false);
        let intent = match hint {
            "operation" => {
                env.request_post_operation_selection(
                    folder.clone(),
                    crate::post_operation_selection::ExpectedOutputs::Known(vec![
                        folder.join("c_1.png"),
                    ]),
                );
                StartupListIntent::ExplicitList
            }
            "startup" => StartupListIntent::RestoreList {
                target: crate::settings::StartupListTarget::PhysicalList {
                    logical_path: folder.clone(),
                    zip_prefix: None,
                },
                cursor: Some(crate::settings::ListCursorHint {
                    name: "c_1.png".into(),
                    rows_above: None,
                }),
            },
            _ => {
                env.select_after_load = Some("c_1.png".into());
                StartupListIntent::ExplicitList
            }
        };
        assert!(env.load_folder_with_scan_owned(
            folder,
            None,
            OpenRequestOwner::Navigation,
            intent
        ));
        env.finish_page_identity_preparation_for_test();
        assert_eq!(env.selected, Some(3), "hint={hint}");
        finish_stack_entry(&mut env);
        assert_eq!(env.selected, Some(2), "hint={hint}");
        assert_eq!(env.grid_scroll.offset(), 0.0);
    }
}

#[test]
fn section1795_off_stack_parent_hint_keeps_passthrough_folder() {
    let (mut env, folder) = stack_entry_fixture(false);
    let child = folder.join("child");
    std::fs::create_dir(folder.join("before-child")).unwrap();
    std::fs::create_dir(&child).unwrap();
    env.settings.facet_filter.exts.clear();
    env.select_after_load = Some("child".into());
    open(&mut env, &folder);
    assert_eq!(
        env.items[env.selected.unwrap()].drag_source_path(),
        Some(child.as_path())
    );
    finish_stack_entry(&mut env);
    assert_eq!(
        env.items[env.selected.unwrap()].drag_source_path(),
        Some(child.as_path())
    );
    assert_eq!(env.grid_scroll.offset(), 0.0);
}

#[test]
fn section1795_off_stack_pending_selection_hidden_by_facet_stays_visible() {
    let (mut env, _) = stack_entry_fixture(false);
    env.settings.facet_filter.exts.clear();
    env.rebuild_visible_indices();
    env.move_grid_cursor(Some(0), GridCursorIntent::Explicit(GridCursorMove::Move));
    // Facet input keeps the cursor before rebuilding visibility. Its previous
    // target must not reselect the hidden JPG stack after aggregate hydration.
    env.move_grid_cursor(Some(0), GridCursorIntent::ExplicitKeep);
    env.settings.facet_filter.exts.insert("png".into());
    env.rebuild_visible_indices();
    finish_stack_entry(&mut env);
    assert_eq!(env.selected, Some(1));
    assert!(env.idx_visible(env.selected.unwrap()));
}

#[test]
fn section1804_stack_pending_empty_facet_then_reset_selects_final_visible_first() {
    for restore in [false, true] {
        let (mut env, folder) = stack_entry_fixture(restore);
        // No saved position: both policies must select the final visible first row.
        env.folder_history.remove(&folder);
        env.settings.facet_filter.exts.clear();
        env.settings.facet_filter.exts.insert("gif".into());
        // Facet handlers capture ExplicitKeep before rebuilding the visible rows.
        let selected = env.selected;
        env.move_grid_cursor(selected, GridCursorIntent::ExplicitKeep);
        env.rebuild_visible_indices();
        assert!(env.current_grid_order().is_empty());
        assert_eq!(env.selected, None);

        env.settings.facet_filter.exts.clear();
        env.move_grid_cursor(None, GridCursorIntent::ExplicitKeep);
        assert!(env.stack_selection_target.is_none());
        env.rebuild_visible_indices();
        assert_eq!(env.selected, Some(0));
        finish_stack_entry(&mut env);
        assert_eq!(env.current_grid_order(), [0, 1, 2]);
        assert_eq!(env.selected, Some(0), "restore={restore}");
        assert_eq!(env.items[0].name(), "a");
        assert!(!env.folder_history.contains_key(&folder));
    }
}

#[test]
fn section1804_stack_hidden_load_hints_do_not_request_deselection() {
    for restore in [false, true] {
        for hint in ["name", "startup"] {
            let (mut env, folder) = stack_entry_fixture(restore);
            env.settings.facet_filter.exts.clear();
            env.settings.facet_filter.exts.insert("gif".into());
            let intent = match hint {
                "startup" => StartupListIntent::RestoreList {
                    target: crate::settings::StartupListTarget::PhysicalList {
                        logical_path: folder.clone(),
                        zip_prefix: None,
                    },
                    cursor: Some(crate::settings::ListCursorHint {
                        name: "a_1.jpg".into(),
                        rows_above: None,
                    }),
                },
                _ => {
                    env.select_after_load = Some("a_1.jpg".into());
                    StartupListIntent::ExplicitList
                }
            };
            assert!(env.load_folder_with_scan_owned(
                folder,
                None,
                OpenRequestOwner::Navigation,
                intent,
            ));
            env.finish_page_identity_preparation_for_test();
            assert!(env.stack_script_pending.is_some());
            assert_eq!(env.selected, None, "restore={restore}, hint={hint}");
            assert!(
                env.stack_selection_target.is_none(),
                "hidden hint is not an explicit clear: restore={restore}, hint={hint}"
            );
            finish_stack_entry(&mut env);
            assert!(env.current_grid_order().is_empty());
            assert_eq!(env.selected, None);
        }
    }
}

#[test]
fn section1795_on_stack_entry_without_history_selects_final_visible_first() {
    let (mut env, folder) = stack_entry_fixture(true);
    // The provisional rows are still this entry, not a departing folder.
    env.folder_history.remove(&folder);
    env.grid_scroll.set_position(125.0);
    finish_stack_entry(&mut env);
    assert!(!env.folder_history.contains_key(&folder));
    assert_position(&env, Some(1), 125.0);
    assert_eq!(env.items[env.selected.unwrap()].name(), "b_1.png");
}

#[test]
fn section1804_stack_reentry_restores_aggregate_history_before_provisional_redirect() {
    let (mut env, folder) = stack_entry_fixture(true);
    finish_stack_entry(&mut env);
    env.move_grid_cursor(Some(1), GridCursorIntent::Explicit(GridCursorMove::Move));
    env.grid_scroll.set_position(175.0);
    open(&mut env, &folder);
    // Aggregate index 1 names b_1.png. On provisional rows it names a hidden JPG,
    // so visibility redirects it to index 2. That correction must not replace history.
    assert_eq!(env.folder_history[&folder], (175.0, Some(1)));
    assert_position(&env, Some(2), 175.0);
    env.grid_scroll.set_position(250.0);
    finish_stack_entry(&mut env);
    assert_eq!(env.folder_history[&folder], (175.0, Some(1)));
    assert_position(&env, Some(1), 250.0);
    assert_eq!(env.items[env.selected.unwrap()].name(), "b_1.png");

    // Only provisional departure was consumed; the installed generation still saves
    // a real departure, including changes made after aggregate adoption.
    env.move_grid_cursor(Some(2), GridCursorIntent::Explicit(GridCursorMove::Move));
    env.grid_scroll.set_position(375.0);
    let other = env.tmp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    open(&mut env, &other);
    assert_eq!(env.folder_history[&folder], (375.0, Some(2)));
}

#[test]
fn section1804_stack_pending_scroll_preserves_on_viewport_without_changing_off_policy() {
    for restore in [false, true] {
        let (mut env, folder) = stack_entry_fixture(restore);
        let history = env.folder_history[&folder];
        assert!(env.stack_selection_target.is_none());
        env.grid_scroll.set_position(250.0);
        finish_stack_entry(&mut env);
        assert_eq!(env.folder_history[&folder], history);
        assert_position(
            &env,
            Some(if restore { 2 } else { 1 }),
            if restore { 250.0 } else { 0.0 },
        );
        if restore {
            assert!(env.grid_scroll.reveal_requested());
        }
    }
}

#[test]
fn section1804_stack_reentry_keeps_existing_unstacked_history_index_semantics() {
    let (mut env, folder) = stack_entry_fixture(true);
    // The fixture's previous visit was unstacked: index 2 named b_1.png. History
    // still stores an index, so applying that record to aggregated rows selects c.
    assert_eq!(env.folder_history[&folder], (420.0, Some(2)));
    env.grid_scroll.set_position(125.0);
    finish_stack_entry(&mut env);
    assert_eq!(env.folder_history[&folder], (420.0, Some(2)));
    assert_position(&env, Some(2), 125.0);
    assert_eq!(env.items[env.selected.unwrap()].name(), "c_1.png");
}

#[test]
fn section1804_stack_toggle_keeps_departure_recording_and_selection() {
    for restore in [false, true] {
        let (mut env, folder) = stack_entry_fixture(restore);
        finish_stack_entry(&mut env);
        env.move_grid_cursor(Some(2), GridCursorIntent::Explicit(GridCursorMove::Move));
        env.grid_scroll.set_position(175.0);
        env.toggle_stack_mode();
        phase_c_folder_nav_history_tests::finish_staged_physical_history_for_test(&mut env);
        env.finish_page_identity_preparation_for_test();
        assert!(!env.stack_mode_on());
        assert_eq!(env.folder_history[&folder], (175.0, Some(2)));
        assert_position(&env, Some(3), 175.0);
        assert_eq!(env.items[env.selected.unwrap()].name(), "c_1.png");

        env.toggle_stack_mode();
        phase_c_folder_nav_history_tests::finish_staged_physical_history_for_test(&mut env);
        env.finish_page_identity_preparation_for_test();
        assert!(env.stack_script_pending.is_some());
        assert_eq!(env.folder_history[&folder], (175.0, Some(3)));
        env.grid_scroll.set_position(250.0);
        finish_stack_entry(&mut env);
        assert_eq!(env.folder_history[&folder], (250.0, Some(3)));
        assert_position(&env, Some(2), 250.0);
        assert_eq!(env.items[env.selected.unwrap()].name(), "c_1.png");
    }
}

#[test]
fn section1804_stack_refresh_keeps_departure_recording_and_selection() {
    for restore in [false, true] {
        let (mut env, folder) = stack_entry_fixture(restore);
        finish_stack_entry(&mut env);
        env.move_grid_cursor(Some(1), GridCursorIntent::Explicit(GridCursorMove::Move));
        env.grid_scroll.set_position(175.0);
        env.reload_current_folder_preserving_override();
        phase_c_folder_nav_history_tests::finish_staged_physical_history_for_test(&mut env);
        env.finish_page_identity_preparation_for_test();
        assert!(env.stack_script_pending.is_some());
        assert_eq!(env.folder_history[&folder], (175.0, Some(1)));
        assert_position(&env, Some(2), 175.0);
        env.grid_scroll.set_position(250.0);
        finish_stack_entry(&mut env);
        assert_eq!(env.folder_history[&folder], (250.0, Some(2)));
        assert_position(&env, Some(1), 250.0);
        assert_eq!(env.items[env.selected.unwrap()].name(), "b_1.png");
    }
}

#[test]
fn section1804_stack_internal_install_keeps_departure_recording() {
    let (mut env, folder) = stack_entry_fixture(true);
    assert!(env.load_folder_with_scan_owned(
        folder.clone(),
        None,
        OpenRequestOwner::Navigation,
        StartupListIntent::InternalInstall,
    ));
    env.finish_page_identity_preparation_for_test();
    env.grid_scroll.set_position(250.0);
    finish_stack_entry(&mut env);
    assert_eq!(env.folder_history[&folder], (250.0, Some(2)));
    assert_position(&env, Some(2), 250.0);
    assert_eq!(env.items[env.selected.unwrap()].name(), "c_1.png");
}

#[test]
fn section1804_subfolder_stack_keeps_session_departure_recording() {
    let (mut env, folder) = stack_entry_fixture(true);
    env.cancel_stack_script_pending();
    env.stack_mode_requested = false;
    env.start_subfolder_expansion_scan_roots(folder.clone(), vec![folder.clone()]);
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !env.items_are_subfolder_expansion_view || env.sidecar_restore_active() {
        assert!(
            std::time::Instant::now() < deadline,
            "subfolder scan did not settle"
        );
        env.poll_subfolder_expansion(&ctx);
        env.finish_page_identity_preparation_for_test();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    env.move_grid_cursor(Some(3), GridCursorIntent::Explicit(GridCursorMove::Move));
    env.toggle_stack_mode();
    assert!(env.stack_script_pending.is_some());
    // The explicit toggle target remains c, while the session departure records
    // the current provisional position. Neither behavior belongs to folder reentry.
    env.move_grid_cursor(Some(2), GridCursorIntent::Automatic);
    env.grid_scroll.set_position(250.0);
    let selected_key = env.rating_path_key(2);
    let key = top_level_grid_view::SubfolderExpansionPositionKey::new(&folder, &[folder.clone()]);
    let history = env.folder_history.clone();
    finish_stack_entry(&mut env);
    let saved = env
        .top_level_grid_view
        .subfolder_expansion_position(&key)
        .unwrap();
    assert_eq!(saved.scroll_offset_y, 250.0);
    assert_eq!(saved.selected_key, selected_key);
    assert_eq!(env.folder_history, history);
    assert!(env.items_are_subfolder_expansion_view);
    assert_eq!(env.items[env.selected.unwrap()].name(), "c_1.png");
}

#[test]
fn section1795_stack_pending_fullscreen_return_keeps_last_page() {
    for restore in [false, true] {
        let (mut env, _) = stack_entry_fixture(restore);
        // Drive the existing close landing boundary without starting a renderer.
        env.fullscreen_idx = Some(3);
        env.restore_grid_cursor_to_fullscreen_item();
        env.fullscreen_idx = None;
        assert_eq!(env.selected, Some(3));
        finish_stack_entry(&mut env);
        assert_eq!(env.selected, Some(2), "restore={restore}");
        assert_eq!(env.items[env.selected.unwrap()].name(), "c_1.png");
    }
}

fn save_reading_position(
    app: &mut App,
    source: &Path,
    page: usize,
) -> crate::book_resume_db::ReadingPosition {
    assert_eq!(app.current_folder.as_deref(), Some(source));
    // Book hydration reads the writer's fenced snapshot. A direct DB::set bypasses
    // its publication and leaves that snapshot unchanged, even after the DB commit.
    app.record_book_resume(page);
    phase_c_support::wait_for_book_resume_write(app, source);
    let key = app.current_book_resume_key().unwrap();
    let saved = app
        .book_resume_db
        .as_ref()
        .unwrap()
        .get_position_key(&key)
        .unwrap();
    assert_eq!(saved.page, page);
    assert!(
        saved.identity.is_some(),
        "use the real reading-position producer"
    );
    let published = app
        .book_resume_writer
        .as_ref()
        .unwrap()
        .position_snapshot()
        .unwrap();
    assert_eq!(published.values.get(&key.storage_key()), Some(&saved));
    saved
}

#[test]
fn section1795_revisit_on_restores_cursor_and_scroll_off_starts_at_first() {
    for restore in [true, false] {
        let (mut env, a, b) = fixture(restore);
        open(&mut env, &b);
        assert_eq!(env.folder_history.get(&a), Some(&(420.0, Some(29))));
        open(&mut env, &a);
        assert_position(
            &env,
            Some(if restore { 29 } else { 0 }),
            if restore { 420.0 } else { 0.0 },
        );
        // Same-path explicit open (favorite/address/tree) is still an entry.
        env.selected = Some(29);
        env.grid_scroll.set_position(420.0);
        open(&mut env, &a);
        assert_position(
            &env,
            Some(if restore { 29 } else { 0 }),
            if restore { 420.0 } else { 0.0 },
        );
    }
}

#[test]
fn section1795_history_back_and_forward_obey_revisit_policy() {
    for restore in [true, false] {
        let (mut env, a, b) = fixture(restore);
        for n in 0..35 {
            image::RgbImage::new(1, 1)
                .save(b.join(format!("{n:02}.png")))
                .unwrap();
        }
        open(&mut env, &b);
        env.selected = Some(29);
        env.grid_scroll.set_position(840.0);
        env.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
        phase_c_folder_nav_history_tests::finish_staged_physical_history_for_test(&mut env);
        assert_eq!(env.current_folder.as_deref(), Some(a.as_path()));
        assert_position(
            &env,
            Some(if restore { 29 } else { 0 }),
            if restore { 420.0 } else { 0.0 },
        );
        env.dispatch_main_folder_history_input(FolderHistoryDirection::Forward);
        phase_c_folder_nav_history_tests::finish_staged_physical_history_for_test(&mut env);
        assert_eq!(env.current_folder.as_deref(), Some(b.as_path()));
        assert_position(
            &env,
            Some(if restore { 29 } else { 0 }),
            if restore { 840.0 } else { 0.0 },
        );
    }
}

#[test]
fn section1795_off_ignores_folder_reading_resume_without_changing_it() {
    for restore in [true, false] {
        let (mut env, a, b) = fixture(restore);
        open(&mut env, &b);
        env.folder_history.remove(&a);
        env.book_resume_db.as_ref().unwrap().set(&a, 29).unwrap();
        open(&mut env, &a);
        assert_eq!(env.selected, Some(if restore { 29 } else { 0 }));
        assert_eq!(env.book_resume_db.as_ref().unwrap().get(&a), Some(29));
        // The fullscreen resolver remains independent of the list policy.
        assert_eq!(env.resume_page_for_container(), Some(29));
        if !restore {
            assert_eq!(env.grid_scroll.offset(), 0.0);
        }
    }
}

#[test]
fn section1795_off_bookshelf_root_starts_first_but_compiled_book_keeps_positions() {
    for restore in [true, false] {
        let (mut env, _, other) = fixture(restore);
        let shelf = env.book_root_path();
        for n in 0..35 {
            std::fs::create_dir_all(shelf.join(format!("book{n:02}"))).unwrap();
        }
        let book = shelf.join("book29");
        for n in 0..35 {
            image::RgbImage::new(1, 1)
                .save(book.join(format!("{n:02}.png")))
                .unwrap();
        }
        env.open_books_root();
        env.finish_page_identity_preparation_for_test();
        assert!(matches!(
            env.projection_recipe,
            book_materialization::ProjectionRecipe::OrdinaryList
        ));
        env.selected = Some(29);
        env.grid_scroll.set_position(420.0);
        open(&mut env, &other);
        env.open_books_root();
        env.finish_page_identity_preparation_for_test();
        assert_eq!(env.current_folder.as_ref(), Some(&shelf));
        assert_position(
            &env,
            Some(if restore { 29 } else { 0 }),
            if restore { 420.0 } else { 0.0 },
        );

        open(&mut env, &book);
        assert!(matches!(
            &env.projection_recipe,
            book_materialization::ProjectionRecipe::Book(
                book_materialization::BookScope::CompiledFolder(path)
            ) if path == &book
        ));
        env.selected = Some(29);
        env.grid_scroll.set_position(420.0);
        open(&mut env, &other);
        open(&mut env, &book);
        assert_position(&env, Some(29), 420.0);
        let saved = save_reading_position(&mut env, &book, 23);
        open(&mut env, &other);
        env.folder_history.remove(&book);
        open(&mut env, &book);
        assert!(matches!(
            &env.projection_recipe,
            book_materialization::ProjectionRecipe::Book(
                book_materialization::BookScope::CompiledFolder(path)
            ) if path == &book
        ));
        assert_eq!(env.selected, Some(23));
        assert_eq!(
            env.book_resume_db.as_ref().unwrap().get_position(&book),
            Some(saved)
        );
    }
}

#[test]
fn section1795_off_parent_backspace_keeps_child_selection_without_history_scroll() {
    let (mut env, a, _) = fixture(false);
    let child = a.join("child");
    std::fs::create_dir(&child).unwrap();
    open(&mut env, &child);
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Backspace,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    let nav = env.handle_keyboard(&ctx);
    ctx.end_pass().drop_without_applying_deltas();
    let Some(crate::ui_main::AddressBarNav::Direct(parent, intent)) = nav else {
        panic!("expected typed parent navigation");
    };
    assert_eq!(parent, a);
    assert!(env.start_physical_history_transition(
        PhysicalHistoryIntent::Navigation {
            replay: None,
            auto_fullscreen: false
        },
        parent,
        intent,
    ));
    phase_c_folder_nav_history_tests::finish_staged_physical_history_for_test(&mut env);
    assert_eq!(
        env.items[env.selected.unwrap()].drag_source_path(),
        Some(child.as_path())
    );
    assert_eq!(env.grid_scroll.offset(), 0.0);
    assert!(env.grid_scroll.reveal_requested());
}

#[test]
fn section1795_off_explicit_and_operation_selection_win_without_history_scroll() {
    for operation in [true, false] {
        let (mut env, a, b) = fixture(false);
        open(&mut env, &b);
        env.select_after_load = Some("28.png".into());
        if operation {
            env.request_post_operation_selection(
                a.clone(),
                crate::post_operation_selection::ExpectedOutputs::Known(vec![a.join("29.png")]),
            );
        }
        open(&mut env, &a);
        assert_position(&env, Some(if operation { 29 } else { 28 }), 0.0);
        assert!(env.grid_scroll.reveal_requested());
        assert!(env.select_after_load.is_none());
    }
}

#[test]
fn section1795_off_refresh_and_sort_preserve_current_position() {
    let (mut env, a, _) = fixture(false);
    env.reload_current_folder_preserving_override();
    phase_c_folder_nav_history_tests::finish_staged_physical_history_for_test(&mut env);
    assert_position(&env, Some(29), 420.0);
    // The production worker sort route uses PreservePresentation and an explicit hint.
    env.settings.sort_order = crate::settings::SortOrder::FileNameDesc;
    env.apply_sort_change_reload_without_ui_io();
    let ready = mutation_refresh_wait_for_order_scan(&mut env);
    mutation_refresh_resolve_order_scan(&mut env, ready);
    env.finish_page_identity_preparation_for_test();
    assert_eq!(env.items[env.selected.unwrap()].name(), "29.png");
    assert_eq!(env.current_folder.as_ref(), Some(&a));
    assert_eq!(env.grid_scroll.offset(), 420.0);
}

#[test]
fn section1795_off_external_rescan_retains_semantic_cursor_and_scroll() {
    let (mut env, a, _) = fixture(false);
    image::RgbImage::new(1, 1)
        .save(a.join("00-first.png"))
        .unwrap();
    let scan = folder_scan::scan_directory_with_settings(&a, &env.settings).unwrap();
    let signature = signature_from_scan(&scan);
    env.apply_external_rescan(a.clone(), std::time::SystemTime::now(), scan, signature);
    env.finish_page_identity_preparation_for_test();
    assert_eq!(env.items.len(), 36, "the changed scan must be adopted");
    assert!(env.items.iter().any(|item| item.name() == "00-first.png"));
    assert_eq!(env.items[env.selected.unwrap()].name(), "29.png");
    assert_eq!(env.grid_scroll.offset(), 420.0);
}

#[test]
fn section1795_off_startup_cursor_remains_independent() {
    use crate::settings::{ListCursorHint, StartupListTarget};
    for restore_last in [true, false] {
        for previous in [true, false] {
            let (mut env, a, b) = fixture(false);
            open(&mut env, &b);
            env.settings.restore_last_cursor = restore_last;
            env.settings.last_folder = Some(a.clone());
            env.settings.last_cursor_name = Some("28.png".into());
            let intent = if previous {
                StartupListIntent::RestoreList {
                    target: StartupListTarget::PhysicalList {
                        logical_path: a.clone(),
                        zip_prefix: None,
                    },
                    cursor: Some(ListCursorHint {
                        name: "28.png".into(),
                        rows_above: Some(2),
                    }),
                }
            } else {
                // Desktop/Specific startup uses this existing explicit hint adapter.
                env.select_after_load =
                    crate::known_folders::startup_cursor_hint(&env.settings, &a)
                        .map(|(name, _)| name)
                        .map(crate::app::AfterLoadSelection::Name);
                StartupListIntent::ExplicitList
            };
            assert!(env.load_folder_with_scan_owned(a, None, OpenRequestOwner::Navigation, intent));
            env.finish_page_identity_preparation_for_test();
            assert_position(&env, Some(if restore_last { 28 } else { 0 }), 0.0);
            if previous && restore_last {
                assert_eq!(env.grid_scroll.row_anchor(), Some(2));
            }
        }
    }
}

#[test]
fn section1795_off_filtered_first_and_empty_lists_match_first_visit() {
    let (mut env, a, b) = fixture(false);
    open(&mut env, &b);
    for n in 0..2 {
        std::fs::remove_file(a.join(format!("{n:02}.png"))).unwrap();
        image::RgbImage::new(1, 1)
            .save(a.join(format!("{n:02}.jpg")))
            .unwrap();
    }
    env.settings.facet_filter.exts.insert("png".into());
    open(&mut env, &a);
    let first = env.current_grid_order()[0];
    assert_eq!(env.selected, Some(first));
    assert_eq!(env.items[first].name(), "02.png");
    assert_eq!(env.grid_scroll.offset(), 0.0);
    open(&mut env, &b);
    assert_position(&env, None, 0.0);
    env.settings.facet_filter.exts.clear();
    env.settings.facet_filter.exts.insert("gif".into());
    open(&mut env, &a);
    assert!(env.visible_indices.is_empty());
    assert_position(&env, None, 0.0);
}

// Drive the common accepted hydration boundary with already enumerated page rows. This avoids
// depending on PDFium/native rendering and still supplies the semantic ExplicitList intent.
fn install_virtual(app: &mut App, source: &Path, pdf: bool) {
    let items: Vec<_> = (0..35)
        .map(|n| {
            if pdf {
                GridItem::PdfPage {
                    pdf_path: source.to_path_buf(),
                    page_num: n,
                    content_type: None,
                }
            } else {
                GridItem::ZipImage {
                    zip_path: source.to_path_buf(),
                    entry_name: format!("{n:02}.png"),
                }
            }
        })
        .collect();
    let token = app.prepare_visible_install(source, &items, None).unwrap();
    let mut hydration = app.install_loading_items_inner(
        source.to_path_buf(),
        items,
        vec![None; 35],
        Default::default(),
        Vec::new(),
        None,
        None,
        None,
        None,
        None,
        None,
        VisibleInstallAuthority::Ordinary,
        token,
        None,
    );
    hydration.as_mut().unwrap().0.restore_intent = StartupListIntent::ExplicitList;
    app.hydrate_installed_main_list(hydration);
    app.finish_page_identity_preparation_for_test();
}

#[test]
fn section1795_off_zip_pdf_keep_session_history_and_reading_resume() {
    for restore in [true, false] {
        for pdf in [true, false] {
            let (mut env, _, b) = fixture(restore);
            let source = env
                .tmp
                .path()
                .join(if pdf { "book.pdf" } else { "book.zip" });
            std::fs::write(&source, b"pre-enumerated fixture").unwrap();
            install_virtual(&mut env, &source, pdf);
            env.selected = Some(29);
            env.grid_scroll.set_position(420.0);
            open(&mut env, &b);
            install_virtual(&mut env, &source, pdf);
            assert_position(&env, Some(29), 420.0);
            let saved = save_reading_position(&mut env, &source, 23);
            open(&mut env, &b);
            env.folder_history.remove(&source);
            install_virtual(&mut env, &source, pdf);
            assert!(matches!(
                env.projection_recipe,
                book_materialization::ProjectionRecipe::Book(_)
            ));
            assert_eq!(env.selected, Some(23));
            assert_eq!(
                env.book_resume_db.as_ref().unwrap().get_position(&source),
                Some(saved)
            );
        }
    }
}

#[test]
fn section1795_off_extracted_directory_retains_existing_history_policy() {
    let (mut env, a, _) = fixture(false);
    let items = env.items.clone();
    let token = env.prepare_visible_install(&a, &items, None).unwrap();
    let mut hydration = env.install_loading_items_inner(
        a,
        items,
        vec![None; 35],
        Default::default(),
        Vec::new(),
        None,
        None,
        None,
        None,
        None,
        None,
        VisibleInstallAuthority::Projection {
            recipe: book_materialization::ProjectionRecipe::ExtractedResult(
                book_materialization::ExtractedSurface::SubfolderExpansion,
            ),
        },
        token,
        None,
    );
    hydration.as_mut().unwrap().0.restore_intent = StartupListIntent::ExplicitList;
    env.hydrate_installed_main_list(hydration);
    env.finish_page_identity_preparation_for_test();
    assert_position(&env, Some(29), 420.0);
}

mod gpu_tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn section1795_preferences_toggle_ok_persists_and_changes_next_load() {
        let (mut env, a, b) = fixture(true);
        open(&mut env, &b);
        env.open_preferences_page(crate::ui_dialogs::preferences::PreferencesPage::Folder);
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1100.0, 1000.0))
            .build_ui_state(|ui, env| env.show_preferences_dialog(ui.ctx()), env);
        harness.run();
        let label = "フォルダに入り直したとき、前回のカーソル位置を復元する";
        harness.get_by_label(label).scroll_to_me();
        harness.run();
        harness.get_by_label(label).click();
        harness.run();
        // Draft edits do not affect the adopted policy until OK.
        assert!(harness.state().settings.restore_folder_cursor_on_revisit);
        harness.get_by_label("  OK  ").click();
        harness.run();
        assert!(!harness.state().show_preferences);
        assert!(!harness.state().settings.restore_folder_cursor_on_revisit);
        assert!(!crate::settings::Settings::load().restore_folder_cursor_on_revisit);
        assert!(crate::settings::Settings::load().restore_last_cursor);
        open(harness.state_mut(), &a);
        assert_position(harness.state(), Some(0), 0.0);
    }
}
