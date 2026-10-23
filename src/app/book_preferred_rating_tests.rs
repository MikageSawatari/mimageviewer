use super::book_materialization::{BookScope, ExtractedSurface, ProjectionRecipe};
use super::book_preferred_rating::{BookPreferredRatingTarget, BookPreferredUnavailable};
use super::tests::phase_c_support::setup_app;
use super::*;
use crate::rating_db::RatingItemKind;

fn zip_nav(path: &Path, names: &[&str]) -> crate::zip_tree::ZipNavState {
    let entries = names
        .iter()
        .map(|name| crate::zip_loader::ZipImageEntry {
            entry_name: (*name).into(),
            uncompressed_size: 0,
            mtime: 0,
            container_ancestry: Vec::new(),
        })
        .collect();
    crate::zip_tree::ZipNavState::new(std::sync::Arc::new(crate::zip_tree::ZipTree::build(
        path.to_path_buf(),
        entries,
    )))
}

fn parent_cell_key(app: &mut App, cell: GridItem) -> String {
    let index = app.items.len();
    app.items.push(cell);
    let key = app.rating_path_key(index).expect("parent cell rating key");
    app.items.pop();
    key
}

fn resolved_book(app: &App) -> super::book_preferred_rating::ContainerRatingTarget {
    match app.resolve_book_preferred_viewer_target() {
        BookPreferredRatingTarget::Book(target) => target,
        _ => panic!("accepted book must resolve to its container"),
    }
}

fn accept_archive(app: &mut App, physical: &Path, logical: &Path, names: &[&str]) {
    app.current_folder = Some(physical.to_path_buf());
    app.archive_source_override = (physical != logical).then(|| logical.to_path_buf());
    app.zip_nav = Some(zip_nav(physical, names));
    accept_archive_level(app, logical);
}

fn accept_archive_level(app: &mut App, logical: &Path) {
    app.projection_recipe = ProjectionRecipe::Book(BookScope::Archive {
        logical_root: logical.to_path_buf(),
        prefix: app.zip_nav.as_ref().unwrap().current().to_vec(),
    });
}

fn install_rows(app: &mut App, items: Vec<GridItem>) {
    app.items = items;
    app.image_metas = vec![None; app.items.len()];
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.visible_indices = (0..app.items.len()).collect();
    app.selected = (!app.items.is_empty()).then_some(0);
    app.settings.write_rating_to_xmp = false;
}

fn bind_action(app: &mut App, action: &str) {
    app.keymap = crate::keymap::Keymap::from_ini_str(&format!("[Rating]\n{action} = F13\n"));
}

fn begin_key(ctx: &egui::Context) {
    ctx.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::F13,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
}

fn grid_key(app: &mut App) {
    let ctx = egui::Context::default();
    begin_key(&ctx);
    let _ = app.handle_keyboard(&ctx);
    ctx.end_pass().drop_without_applying_deltas();
}

fn viewer_key(app: &mut App) {
    let ctx = egui::Context::default();
    begin_key(&ctx);
    let index = app.fullscreen_idx.expect("viewer is open");
    let _ = app.handle_fs_key_input(&ctx, index, false);
    ctx.end_pass().drop_without_applying_deltas();
}

#[test]
fn book_preferred_ordinary_and_all_extracted_surfaces_use_viewer_item() {
    use super::top_level_grid_view::TopLevelSearchView;
    let mut app = setup_app();
    let old_book = app.tmp.path().join("old-book");
    app.current_folder = Some(old_book);
    let mut recipes = vec![ProjectionRecipe::OrdinaryList];
    recipes.extend(
        [
            ExtractedSurface::Search(TopLevelSearchView::Global),
            ExtractedSurface::Search(TopLevelSearchView::Favorite),
            ExtractedSurface::Search(TopLevelSearchView::Tag),
            ExtractedSurface::Rating,
            ExtractedSurface::SmartFolder,
            ExtractedSurface::Collection,
            ExtractedSurface::Snapshot,
            ExtractedSurface::SubfolderExpansion,
        ]
        .map(ProjectionRecipe::ExtractedResult),
    );
    for recipe in recipes {
        app.projection_recipe = recipe.clone();
        assert!(
            matches!(
                app.resolve_book_preferred_viewer_target(),
                BookPreferredRatingTarget::ViewerItem
            ),
            "{recipe:?}"
        );
    }
}

#[test]
fn book_preferred_zip_root_matches_parent_zip_cell_without_existing_file() {
    let mut app = setup_app();
    let path = app.tmp.path().join("absent.zip");
    assert!(!path.exists());
    let expected = parent_cell_key(&mut app, GridItem::ZipFile(path.clone()));
    accept_archive(&mut app, &path, &path, &["01.jpg", "02.jpg"]);
    let target = resolved_book(&app);
    assert_eq!(target.key, expected);
    assert_eq!(target.source, path);
    assert_eq!(target.meta.kind, RatingItemKind::ZipFile);
}

#[test]
fn book_preferred_initial_zip_collapse_stays_outer_book() {
    let mut app = setup_app();
    let path = app.tmp.path().join("wrapper.zip");
    let expected = parent_cell_key(&mut app, GridItem::ZipFile(path.clone()));
    accept_archive(&mut app, &path, &path, &["wrapper/01.jpg"]);
    let nav = app.zip_nav.as_ref().unwrap();
    assert!(nav.at_root());
    assert!(!nav.current().is_empty());
    let target = resolved_book(&app);
    assert_eq!(target.key, expected);
    assert_eq!(target.meta.kind, RatingItemKind::ZipFile);
    assert!(target.meta.dir_prefix.is_none());
}

#[test]
fn book_preferred_nested_zip_collapse_matches_literal_parent_zipdir_cell() {
    let mut app = setup_app();
    let path = app.tmp.path().join("nested.zip");
    accept_archive(
        &mut app,
        &path,
        &path,
        &["bookA/01.jpg", "bookB/only/01.jpg"],
    );
    let expected = parent_cell_key(
        &mut app,
        GridItem::ZipDir {
            zip_path: path.clone(),
            dir_prefix: "bookB/".into(),
            is_archive: false,
            representative: Some("bookB/only/01.jpg".into()),
        },
    );
    app.zip_nav.as_mut().unwrap().enter("bookB/");
    accept_archive_level(&mut app, &path);
    let nav = app.zip_nav.as_ref().unwrap();
    assert!(!nav.at_root());
    assert_eq!(nav.current(), &["bookB".to_string(), "only".to_string()]);
    let target = resolved_book(&app);
    assert_eq!(target.key, expected);
    assert_ne!(
        target.key,
        crate::path_key::normalize(&path.join("bookB/only"))
    );
    assert_eq!(target.meta.kind, RatingItemKind::ZipDir);
    assert_eq!(target.meta.dir_prefix.as_deref(), Some("bookB/"));
    assert_eq!(target.meta.zipdir_is_archive, Some(false));
}

#[test]
fn book_preferred_converted_archive_matches_original_parent_cell_at_both_levels() {
    let mut app = setup_app();
    let cache = app.tmp.path().join("cache.zip");
    let source = app.tmp.path().join("source.rar");
    let expected = parent_cell_key(
        &mut app,
        GridItem::ConvertibleArchive {
            path: source.clone(),
            format: crate::archive_converter::ArchiveFormat::Rar,
        },
    );
    accept_archive(
        &mut app,
        &cache,
        &source,
        &["other/01.jpg", "inner.zip/wrapper/01.jpg"],
    );
    let target = resolved_book(&app);
    assert_eq!(target.key, expected);
    assert_eq!(target.source, source);
    assert_eq!(target.meta.kind, RatingItemKind::ConvertibleArchive);
    assert!(target.meta.archive_format.is_some());
    let expected = parent_cell_key(
        &mut app,
        GridItem::ZipDir {
            zip_path: cache.clone(),
            dir_prefix: "inner.zip/".into(),
            is_archive: true,
            representative: None,
        },
    );
    app.zip_nav.as_mut().unwrap().enter("inner.zip/");
    accept_archive_level(&mut app, &source);
    let target = resolved_book(&app);
    assert_eq!(target.key, expected);
    assert_eq!(target.source, source);
    assert_eq!(target.meta.kind, RatingItemKind::ZipDir);
    assert_eq!(target.meta.dir_prefix.as_deref(), Some("inner.zip/"));
    assert_eq!(target.meta.zipdir_is_archive, Some(true));
}

#[test]
fn book_preferred_pdf_and_epub_match_logical_document_parent_cell() {
    let mut app = setup_app();
    for extension in ["pdf", "epub"] {
        let logical = app.tmp.path().join(format!("absent.{extension}"));
        assert!(!logical.exists());
        let expected = parent_cell_key(&mut app, GridItem::PdfFile(logical.clone()));
        app.current_folder = Some(logical.clone());
        app.projection_recipe = ProjectionRecipe::Book(BookScope::Document(logical.clone()));
        let target = resolved_book(&app);
        assert_eq!(target.key, expected);
        assert_eq!(target.source, logical);
        assert_eq!(target.meta.kind, RatingItemKind::PdfFile);
    }
}

#[test]
fn book_preferred_image_and_compiled_folders_match_parent_folder_even_when_absent() {
    let mut app = setup_app();
    // A suffix that looks like a document must still use the accepted folder kind.
    let path = app.tmp.path().join("absent-folder.pdf");
    let expected = parent_cell_key(&mut app, GridItem::Folder(path.clone()));
    assert!(!path.exists());
    app.current_folder = Some(path.clone());
    for scope in [
        BookScope::ImageFolder(path.clone()),
        BookScope::CompiledFolder(path.clone()),
    ] {
        app.projection_recipe = ProjectionRecipe::Book(scope);
        let target = resolved_book(&app);
        assert_eq!(target.key, expected);
        assert_eq!(target.source, path);
        assert_eq!(target.meta.kind, RatingItemKind::Folder);
    }
}

#[test]
fn book_preferred_search_open_result_adoption_and_book_drill_have_distinct_targets() {
    let mut app = setup_app();
    let old_book = app.tmp.path().join("before-search");
    let new_book = app.tmp.path().join("search-result.zip");
    let old_key = parent_cell_key(&mut app, GridItem::Folder(old_book.clone()));
    let new_key = parent_cell_key(&mut app, GridItem::ZipFile(new_book.clone()));
    app.current_folder = Some(old_book.clone());
    app.projection_recipe = ProjectionRecipe::Book(BookScope::ImageFolder(old_book));
    // Ctrl+G has opened, but its result has not replaced the accepted book yet.
    app.global_search.active = true;
    assert_eq!(resolved_book(&app).key, old_key);
    assert!(app.current_container_rating_key_and_source().is_none());
    app.projection_recipe = ProjectionRecipe::ExtractedResult(ExtractedSurface::Search(
        super::top_level_grid_view::TopLevelSearchView::Global,
    ));
    assert!(matches!(
        app.resolve_book_preferred_viewer_target(),
        BookPreferredRatingTarget::ViewerItem
    ));
    accept_archive(&mut app, &new_book, &new_book, &["01.jpg"]);
    let target = resolved_book(&app);
    assert_eq!(target.key, new_key);
    assert_ne!(target.key, old_key);
    assert!(app.global_search.active);
}

#[test]
fn book_preferred_rejects_missing_and_mismatched_physical_state_with_typed_reason() {
    let mut app = setup_app();
    let path = app.tmp.path().join("book.zip");
    let other = app.tmp.path().join("other.zip");
    app.current_folder = None;
    app.projection_recipe = ProjectionRecipe::Book(BookScope::ImageFolder(path.clone()));
    assert!(matches!(
        app.resolve_book_preferred_viewer_target(),
        BookPreferredRatingTarget::Unavailable(BookPreferredUnavailable::NoPhysicalTarget)
    ));
    app.current_folder = Some(search_results_synthetic_path());
    assert!(matches!(
        app.resolve_book_preferred_viewer_target(),
        BookPreferredRatingTarget::Unavailable(BookPreferredUnavailable::NoPhysicalTarget)
    ));
    accept_archive(&mut app, &path, &path, &["01.jpg"]);
    app.projection_recipe = ProjectionRecipe::Book(BookScope::Archive {
        logical_root: other.clone(),
        prefix: vec![],
    });
    assert!(matches!(
        app.resolve_book_preferred_viewer_target(),
        BookPreferredRatingTarget::Unavailable(BookPreferredUnavailable::ArchiveRootMismatch)
    ));
    app.projection_recipe = ProjectionRecipe::Book(BookScope::Archive {
        logical_root: path.clone(),
        prefix: vec!["wrong-level".into()],
    });
    assert!(matches!(
        app.resolve_book_preferred_viewer_target(),
        BookPreferredRatingTarget::Unavailable(BookPreferredUnavailable::ArchiveLevelMismatch)
    ));
    app.projection_recipe = ProjectionRecipe::Book(BookScope::Document(path.clone()));
    assert!(matches!(
        app.resolve_book_preferred_viewer_target(),
        BookPreferredRatingTarget::Unavailable(BookPreferredUnavailable::DocumentMismatch)
    ));
    for scope in [
        BookScope::ImageFolder(path.clone()),
        BookScope::CompiledFolder(path.clone()),
    ] {
        app.projection_recipe = ProjectionRecipe::Book(scope);
        assert!(matches!(
            app.resolve_book_preferred_viewer_target(),
            BookPreferredRatingTarget::Unavailable(BookPreferredUnavailable::FolderMismatch)
        ));
    }
    app.zip_nav = None;
    app.projection_recipe = ProjectionRecipe::Book(BookScope::Archive {
        logical_root: path.clone(),
        prefix: vec![],
    });
    assert!(matches!(
        app.resolve_book_preferred_viewer_target(),
        BookPreferredRatingTarget::Unavailable(BookPreferredUnavailable::ArchiveRootMismatch)
    ));
    app.projection_recipe = ProjectionRecipe::Book(BookScope::Document(other.clone()));
    assert!(matches!(
        app.resolve_book_preferred_viewer_target(),
        BookPreferredRatingTarget::Unavailable(BookPreferredUnavailable::DocumentMismatch)
    ));
    for scope in [
        BookScope::ImageFolder(other.clone()),
        BookScope::CompiledFolder(other),
    ] {
        app.projection_recipe = ProjectionRecipe::Book(scope);
        assert!(matches!(
            app.resolve_book_preferred_viewer_target(),
            BookPreferredRatingTarget::Unavailable(BookPreferredUnavailable::FolderMismatch)
        ));
    }
}

#[test]
fn book_preferred_grid_single_and_checked_batch_target_cells_with_one_undo() {
    let _input_guard = crate::key_input::lock_test_input();
    let mut app = setup_app();
    let parent = app.tmp.path().join("shelf");
    install_rows(
        &mut app,
        vec![
            GridItem::Image(parent.join("page.jpg")),
            GridItem::ZipFile(parent.join("book.zip")),
            GridItem::Folder(parent.join("folder")),
        ],
    );
    app.current_folder = Some(parent);
    let keys: Vec<_> = (0..3)
        .map(|index| app.rating_path_key(index).unwrap())
        .collect();
    bind_action(&mut app, "RatingBookPreferred2");
    grid_key(&mut app);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&keys[0]), 2);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&keys[1]), 0);
    app.apply_meta_undo();
    assert_eq!(app.rating_db.as_ref().unwrap().get(&keys[0]), 0);
    app.checked.extend([0, 1, 2]);
    bind_action(&mut app, "RatingBookPreferredStepUp");
    grid_key(&mut app);
    for key in &keys {
        assert_eq!(app.rating_db.as_ref().unwrap().get(key), 4);
    }
    assert_eq!(app.meta_undo.undo_len(), 1);
    let crate::undo_stack::UndoEntry::Rating { changes, .. } = app.meta_undo.peek_undo().unwrap()
    else {
        panic!("rating undo");
    };
    assert_eq!(changes.len(), 3);
    app.apply_meta_undo();
    for key in &keys {
        assert_eq!(app.rating_db.as_ref().unwrap().get(key), 0);
    }
    assert_eq!(app.meta_undo.undo_len(), 0);
}

#[test]
fn book_preferred_book_page_grid_rates_page_while_viewer_rates_book() {
    let _input_guard = crate::key_input::lock_test_input();
    let mut app = setup_app();
    let book = app.tmp.path().join("pages.pdf");
    let book_key = parent_cell_key(&mut app, GridItem::PdfFile(book.clone()));
    app.current_folder = Some(book.clone());
    app.projection_recipe = ProjectionRecipe::Book(BookScope::Document(book.clone()));
    install_rows(
        &mut app,
        vec![GridItem::PdfPage {
            pdf_path: book,
            page_num: 0,
            content_type: None,
        }],
    );
    app.finish_page_identity_preparation_for_test();
    let page_key = app.rating_path_key(0).unwrap();
    bind_action(&mut app, "RatingBookPreferred3");
    grid_key(&mut app);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&page_key), 3);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&book_key), 0);
    app.fullscreen_idx = Some(0);
    bind_action(&mut app, "RatingBookPreferred5");
    viewer_key(&mut app);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&page_key), 3);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&book_key), 5);
    assert!(
        app.fs_feedback_toast
            .as_ref()
            .is_some_and(|(message, _, _)| message.contains("本"))
    );
}

#[test]
fn book_preferred_nonbook_viewer_step_and_bound_change_only_current_item() {
    let _input_guard = crate::key_input::lock_test_input();
    let mut app = setup_app();
    let folder = app.tmp.path().join("ordinary");
    let folder_key = parent_cell_key(&mut app, GridItem::Folder(folder.clone()));
    app.current_folder = Some(folder.clone());
    install_rows(
        &mut app,
        vec![
            GridItem::Image(folder.join("page.jpg")),
            GridItem::Image(folder.join("other.jpg")),
        ],
    );
    app.projection_recipe = ProjectionRecipe::OrdinaryList;
    app.fullscreen_idx = Some(0);
    app.finish_page_identity_preparation_for_test();
    let page_key = app.rating_path_key(0).unwrap();
    let other_key = app.rating_path_key(1).unwrap();
    bind_action(&mut app, "RatingBookPreferredStepUp");
    viewer_key(&mut app);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&page_key), 4);
    viewer_key(&mut app);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&page_key), 5);
    let generation = app.rating_session_write_generation;
    let undo_count = app.meta_undo.undo_len();
    viewer_key(&mut app);
    assert_eq!(app.rating_session_write_generation, generation);
    assert_eq!(app.meta_undo.undo_len(), undo_count);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&folder_key), 0);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&other_key), 0);
}

#[test]
fn book_preferred_book_viewer_step_bound_and_clear_preserve_page() {
    let _input_guard = crate::key_input::lock_test_input();
    let mut app = setup_app();
    let folder = app.tmp.path().join("image-book");
    app.current_folder = Some(folder.clone());
    app.projection_recipe = ProjectionRecipe::Book(BookScope::ImageFolder(folder.clone()));
    install_rows(&mut app, vec![GridItem::Image(folder.join("page.jpg"))]);
    app.finish_page_identity_preparation_for_test();
    app.fullscreen_idx = Some(0);
    let page_key = app.rating_path_key(0).unwrap();
    let book_key = resolved_book(&app).key;
    bind_action(&mut app, "RatingBookPreferredStepUp");
    viewer_key(&mut app);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&book_key), 4);
    viewer_key(&mut app);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&book_key), 5);
    let generation = app.rating_session_write_generation;
    viewer_key(&mut app);
    assert_eq!(app.rating_session_write_generation, generation);
    assert_eq!(app.meta_undo.undo_len(), 2);
    bind_action(&mut app, "RatingBookPreferredClear");
    viewer_key(&mut app);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&book_key), 0);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&page_key), 0);
    app.apply_meta_undo();
    assert_eq!(app.rating_db.as_ref().unwrap().get(&book_key), 5);
}

#[test]
fn book_preferred_unavailable_viewer_never_falls_back_to_page_or_adds_undo() {
    let _input_guard = crate::key_input::lock_test_input();
    let mut app = setup_app();
    let folder = app.tmp.path().join("physical");
    install_rows(&mut app, vec![GridItem::Image(folder.join("page.jpg"))]);
    app.current_folder = Some(folder);
    app.projection_recipe =
        ProjectionRecipe::Book(BookScope::ImageFolder(app.tmp.path().join("different")));
    app.fullscreen_idx = Some(0);
    app.finish_page_identity_preparation_for_test();
    let page_key = app.rating_path_key(0).unwrap();
    bind_action(&mut app, "RatingBookPreferred5");
    viewer_key(&mut app);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&page_key), 0);
    assert_eq!(app.meta_undo.undo_len(), 0);
    assert!(
        app.fs_feedback_toast
            .as_ref()
            .is_some_and(|(message, _, _)| message == "[この本は評価できません]")
    );
}

#[test]
fn book_preferred_item_filter_departure_applies_once_and_one_undo_restores_value() {
    let _input_guard = crate::key_input::lock_test_input();
    let mut app = setup_app();
    let path = app.tmp.path().join("filter-page.jpg");
    install_rows(&mut app, vec![GridItem::Image(path)]);
    app.projection_recipe = ProjectionRecipe::OrdinaryList;
    app.fullscreen_idx = Some(0);
    app.finish_page_identity_preparation_for_test();
    let key = app.rating_path_key(0).unwrap();
    app.rating_db.as_ref().unwrap().set(&key, 4).unwrap();
    app.rating_cache.insert(0, 4);
    app.settings.rating_filter = [false, false, false, false, true, false];
    bind_action(&mut app, "RatingBookPreferredStepUp");
    let ctx = egui::Context::default();
    begin_key(&ctx);
    let _ = app.handle_fs_key_input(&ctx, 0, false);
    // Re-entering a handler in the same pass cannot reuse the departing press.
    let _ = app.handle_keyboard(&ctx);
    ctx.end_pass().drop_without_applying_deltas();
    assert_eq!(app.rating_db.as_ref().unwrap().get(&key), 5);
    assert!(app.visible_indices.is_empty());
    assert_eq!(app.meta_undo.undo_len(), 1);
    app.apply_meta_undo();
    assert_eq!(app.rating_db.as_ref().unwrap().get(&key), 4);
    assert_eq!(app.meta_undo.undo_len(), 0);
}

#[test]
fn book_preferred_book_cell_filter_departure_is_one_edit_and_one_undo() {
    let _input_guard = crate::key_input::lock_test_input();
    let mut app = setup_app();
    let path = app.tmp.path().join("filtered-book.zip");
    install_rows(&mut app, vec![GridItem::ZipFile(path)]);
    let key = app.rating_path_key(0).unwrap();
    app.rating_db.as_ref().unwrap().set(&key, 4).unwrap();
    app.rating_cache.insert(0, 4);
    app.settings.rating_filter = [false, false, false, false, true, false];
    bind_action(&mut app, "RatingBookPreferredStepUp");
    let ctx = egui::Context::default();
    begin_key(&ctx);
    let _ = app.handle_keyboard(&ctx);
    let _ = app.handle_keyboard(&ctx);
    ctx.end_pass().drop_without_applying_deltas();
    assert_eq!(app.rating_db.as_ref().unwrap().get(&key), 5);
    assert!(app.visible_indices.is_empty());
    assert_eq!(app.meta_undo.undo_len(), 1);
    app.apply_meta_undo();
    assert_eq!(app.rating_db.as_ref().unwrap().get(&key), 4);
    assert_eq!(app.meta_undo.undo_len(), 0);
}

#[test]
fn book_preferred_search_book_step_uses_database_instead_of_address_bar_zero_cache() {
    let _input_guard = crate::key_input::lock_test_input();
    let mut app = setup_app();
    let book = app.tmp.path().join("searched-book.zip");
    accept_archive(&mut app, &book, &book, &["01.jpg"]);
    install_rows(
        &mut app,
        vec![GridItem::ZipImage {
            zip_path: book.clone(),
            entry_name: "01.jpg".into(),
        }],
    );
    app.finish_page_identity_preparation_for_test();
    app.fullscreen_idx = Some(0);
    app.global_search.active = true;
    let book_key = resolved_book(&app).key;
    let page_key = app.rating_path_key(0).unwrap();
    app.rating_db.as_ref().unwrap().set(&book_key, 2).unwrap();
    app.current_folder_rating_cache = Some(0);
    bind_action(&mut app, "RatingBookPreferredStepUp");
    viewer_key(&mut app);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&book_key), 3);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&page_key), 0);
    assert_eq!(app.current_folder_rating_cache, Some(0));
    app.apply_meta_undo();
    assert_eq!(app.rating_db.as_ref().unwrap().get(&book_key), 2);
}

#[test]
fn book_preferred_detached_input_uses_mounted_book_and_preserves_unrelated_main_state() {
    let _input_guard = crate::key_input::lock_test_input();
    let mut app = setup_app();
    let main = app.tmp.path().join("main-B");
    let book = app.tmp.path().join("detached-A");
    install_rows(&mut app, vec![GridItem::Image(main.join("main.jpg"))]);
    app.current_folder = Some(main.clone());
    app.projection_recipe = ProjectionRecipe::OrdinaryList;
    app.rating_cache.insert(0, 2);
    app.current_folder_rating_cache = Some(3);
    let main_key = app.rating_path_key(0).unwrap();
    let book_key = parent_cell_key(&mut app, GridItem::Folder(book.clone()));
    bind_action(&mut app, "RatingBookPreferred4");
    app.build_active_context_for_test(None, DetachedSource::Image, |detached| {
        detached.current_folder = Some(book.clone());
        detached.projection_recipe = ProjectionRecipe::Book(BookScope::ImageFolder(book.clone()));
        install_rows(detached, vec![GridItem::Image(book.join("page.jpg"))]);
        detached.fullscreen_idx = Some(0);
        detached.viewer_presentation = ViewerPresentation::DetachedWindow;
        detached.detached_viewer_independent_active = true;
        detached.finish_page_identity_preparation_for_test();
    });
    let main_items = app.items.clone();
    let main_ratings = app.rating_cache.clone();
    let main_container_rating = app.current_folder_rating_cache;
    app.with_active_viewer_context(|detached| {
        assert_eq!(resolved_book(detached).key, book_key);
        viewer_key(detached);
    })
    .expect("mounted detached book");
    assert_eq!(app.rating_db.as_ref().unwrap().get(&book_key), 4);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&main_key), 0);
    assert_eq!(app.items, main_items);
    assert_eq!(app.rating_cache, main_ratings);
    assert_eq!(app.current_folder_rating_cache, main_container_rating);
    assert!(matches!(
        app.resolve_book_preferred_viewer_target(),
        BookPreferredRatingTarget::ViewerItem
    ));
    grid_key(&mut app);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&main_key), 4);
    assert_eq!(app.rating_db.as_ref().unwrap().get(&book_key), 4);
}
