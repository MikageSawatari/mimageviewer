//! Physical-parent preview and production input/adoption regressions. No application windows.
use super::*;
use crate::app::grid_parent::{GridParentAction, GridPhysicalParentAction};
use crate::ring_shortcut::{MouseGestureBinding, MouseGestureDirection, RightDragContext};
use crate::ui_dialogs::context_menu::{JumpToFolderDestination, JumpToFolderSelection};
use crate::ui_main::AddressBarNav;

fn assert_jump(app: &App, source: &Path) {
    let action = app.grid_physical_parent_action();
    assert!(action.available());
    let GridPhysicalParentAction::Jump(request) = action else {
        panic!("expected physical jump from {source:?}: {action:?}");
    };
    assert_eq!(
        request.destination,
        JumpToFolderDestination::PhysicalDirectory(source.parent().unwrap().to_owned())
    );
    assert_eq!(
        request.selection,
        JumpToFolderSelection::ExactPath(source.to_owned())
    );
    assert!(request.origin.is_none());
}

fn state(app: &App) -> String {
    format!(
        "{:?}/{:?}/{:?}/{:?}/{:?}/{:?}/{:?}/{:?}/{:?}/{:?}",
        app.current_folder,
        app.archive_source_override,
        app.selected,
        app.select_after_load,
        app.items,
        app.zip_nav.as_ref().map(|nav| nav.current()),
        app.reading_history_return_from,
        app.folder_nav_back_stack,
        app.folder_nav_forward_stack,
        app.projection_recipe
    )
}

#[test]
fn section1672_resolver_outer_source_is_readonly_for_folders_documents_and_nested_archives() {
    let mut app = setup(false);
    configure(&mut app);
    // None of these paths exists: preview must not stat/read/convert anything.
    for name in [
        "ordinary",
        "image_book",
        "book.pdf",
        "book.epub",
        "book.rar",
        "book.7z",
        "book.lzh",
    ] {
        let source = crate::data_dir::get()
            .join("nonexistent-library")
            .join(name);
        app.current_folder = Some(source.clone());
        app.archive_source_override = None;
        app.zip_nav = None;
        if name.ends_with("epub")
            || name.ends_with("rar")
            || name.ends_with("7z")
            || name.ends_with("lzh")
        {
            app.current_folder =
                Some(crate::data_dir::get().join("archive_cache/internal/book.zip"));
            app.archive_source_override = Some(source.clone());
        }
        let before = state(&app);
        assert_jump(&app, &source);
        assert!(
            !app.grid_physical_parent_action()
                .tooltip()
                .contains("archive_cache")
        );
        assert_eq!(before, state(&app));
        assert!(app.folder_pane_open_pending.is_none());
    }
    let source = crate::data_dir::get().join("nonexistent-library/outer.zip");
    let tree = std::sync::Arc::new(crate::zip_tree::ZipTree::build(
        source.clone(),
        vec![crate::zip_loader::ZipImageEntry {
            entry_name: "chapter/inner.zip/deep/page.png".into(),
            uncompressed_size: 0,
            mtime: 0,
            container_ancestry: Vec::new(),
        }],
    ));
    app.current_folder = Some(source.clone());
    app.archive_source_override = None;
    app.zip_nav = Some(crate::zip_tree::ZipNavState::new(tree));
    for prefix in ["", "chapter/", "chapter/inner.zip/deep/"] {
        if !prefix.is_empty() {
            app.zip_nav.as_mut().unwrap().enter(prefix);
        }
        let before = state(&app);
        assert_jump(&app, &source);
        assert_eq!(before, state(&app));
    }
    // A converted outer archive retains the same tree but selects its original file.
    let original = source.with_extension("7z");
    app.archive_source_override = Some(original.clone());
    assert_jump(&app, &original);
    app.zip_nav = None;
    app.archive_source_override = None;
    app.current_folder = Some(PathBuf::from(r"C:\"));
    assert_eq!(
        app.grid_physical_parent_action(),
        GridPhysicalParentAction::DriveList(PathBuf::from(r"C:\"))
    );
}

#[test]
fn section1672_resolver_rejects_all_synthetic_roots_even_with_a_stale_physical_path() {
    use crate::app::book_materialization::{ExtractedSurface, ProjectionRecipe};
    use crate::app::top_level_grid_view::TopLevelSearchView;
    let mut app = setup(false);
    configure(&mut app);
    app.current_folder = Some(crate::data_dir::get().join("stale/physical"));
    for kind in [
        "history",
        "global",
        "favorite",
        "tag",
        "rating",
        "bookmarks",
        "drives",
        "subfolder",
        "smart",
    ] {
        app.items_are_reading_history_view = kind == "history";
        app.items_are_global_search_view = kind == "global";
        app.favsearch.active = kind == "favorite";
        app.items_are_tag_view = kind == "tag";
        app.items_are_rating_view = kind == "rating";
        app.items_are_bookmark_view = kind == "bookmarks";
        app.items_are_drive_list = kind == "drives";
        app.items_are_subfolder_expansion_view = kind == "subfolder";
        app.projection_recipe = if kind == "smart" {
            ProjectionRecipe::ExtractedResult(ExtractedSurface::SmartFolder)
        } else {
            ProjectionRecipe::OrdinaryList
        };
        let before = state(&app);
        assert!(!app.grid_physical_parent_action().available(), "{kind}");
        assert!(app.handle_grid_physical_parent_action().is_none());
        assert!(app.folder_pane_open_pending.is_none());
        assert_eq!(before, state(&app));
    }
    app.items_are_subfolder_expansion_view = false;
    for surface in [
        ExtractedSurface::Search(TopLevelSearchView::Global),
        ExtractedSurface::Collection,
        ExtractedSurface::Snapshot,
        ExtractedSurface::SmartFolder,
        ExtractedSurface::Rating,
        ExtractedSurface::SubfolderExpansion,
    ] {
        app.projection_recipe = ProjectionRecipe::ExtractedResult(surface);
        assert!(
            !app.grid_physical_parent_action().available(),
            "{surface:?}"
        );
    }
}

fn input(app: &mut App, route: &str) -> Option<AddressBarNav> {
    let ctx = egui::Context::default();
    match route {
        "key" => {
            app.keymap =
                crate::keymap::Keymap::from_ini_str("[Grid]\nGridPhysicalParentFolder = F14\n");
            key(app, egui::Key::F14, egui::Modifiers::NONE)
        }
        "mouse" => {
            app.settings
                .ring_shortcuts
                .mouse_button_profile_mut(RingShortcutContext::Grid)
                .back = RingActionId::GridPhysicalParentFolder;
            app.apply_mouse_button(
                &ctx,
                MouseButtonSlot::Back,
                ActionSurface::MainWindow,
                "1672",
            )
        }
        "ring" => app.apply_ring_action(
            &ctx,
            RingShortcutContext::Grid,
            RingActionId::GridPhysicalParentFolder,
            "1672",
        ),
        "gesture" => {
            app.settings.ring_shortcuts.mouse_gestures_grid.bindings =
                vec![MouseGestureBinding::new(
                    vec![MouseGestureDirection::Up],
                    RingActionId::GridPhysicalParentFolder,
                )];
            app.trigger_mouse_gesture_action(
                &ctx,
                RightDragContext::Grid,
                &[MouseGestureDirection::Up],
            )
        }
        "button" => {
            use egui_kittest::kittest::Queryable;
            let mut nav = None;
            let fonts = std::cell::Cell::new(false);
            app.settings.show_toolbar_physical_parent_button = true;
            app.settings.toolbar_section_order = vec![crate::settings::ToolbarSectionId::Folder];
            {
                let mut harness = crate::test_ui::builder()
                    .with_size(egui::vec2(1400.0, 240.0))
                    .build_ui(|ui| {
                        if !fonts.replace(true) {
                            crate::ui_fonts::configure_fonts(ui.ctx());
                            ui.ctx().request_repaint();
                            return;
                        }
                        if let Some(next) = app.render_toolbar(ui).1 {
                            nav = Some(next);
                        }
                    });
                harness.run();
                harness.get_by_label("実際の親フォルダへ移動").click();
                harness.run();
            }
            nav
        }
        _ => unreachable!(),
    }
}

fn finish_jump(app: &mut App) {
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.folder_pane_open_pending.is_some() {
        if let Some(ready) = app.poll_folder_pane_open(&ctx) {
            assert!(app.resolve_main_folder_open_ready(&ctx, ready).is_none());
        }
        assert!(
            Instant::now() < deadline,
            "physical jump scan did not settle"
        );
        std::thread::yield_now();
    }
    settle(app);
}

fn nested_zip(path: &Path) {
    use std::io::Write;
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(3, 4)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let mut inner = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    inner
        .start_file("deep/page.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    inner.write_all(png.get_ref()).unwrap();
    let bytes = inner.finish().unwrap().into_inner();
    let mut outer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for name in ["page.png", "chapter/page.png"] {
        outer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        outer.write_all(png.get_ref()).unwrap();
    }
    outer
        .start_file(
            "chapter/inner.zip",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    outer.write_all(&bytes).unwrap();
    outer.finish().unwrap();
}

#[test]
fn section1672_all_inputs_use_the_same_preview_and_exact_jump_selection() {
    #[cfg(windows)]
    let _focus = crate::test_native_focus::scoped(1, 1, true);
    for route in ["key", "mouse", "ring", "gesture", "button"] {
        let mut app = setup(false);
        configure(&mut app);
        let (source, _) = fixture(&mut app, "folder");
        app.load_folder(source.clone());
        settle(&mut app);
        let before = state(&app);
        assert_jump(&app, &source);
        assert_eq!(before, state(&app));
        assert!(input(&mut app, route).is_none());
        let pending = app.folder_pane_open_pending.as_ref().expect(route);
        assert_eq!(pending.path, source.parent().unwrap());
        assert!(
            matches!(&pending.purpose, FolderOpenScanPurpose::JumpToPhysicalFolder { selection: JumpToFolderSelection::ExactPath(path), origin: None } if path == &source)
        );
        finish_jump(&mut app);
        assert_eq!(app.current_folder.as_deref(), source.parent());
        assert!(
            app.selected
                .is_some_and(|index| app.items[index].container_path() == Some(source.as_path())),
            "{route}"
        );
    }
}

#[test]
fn section1672_history_and_search_books_exit_to_physical_list_without_old_return() {
    #[cfg(windows)]
    let _focus = crate::test_native_focus::scoped(1, 1, true);
    for entrance in ["history", "favorite", "global"] {
        for kind in ["folder", "image_book", "zip", "pdf", "archive", "epub"] {
            let mut app = setup(false);
            configure(&mut app);
            let (source, history_kind) = fixture(&mut app, kind);
            if kind == "zip" {
                nested_zip(&source);
            }
            let parent = source.parent().unwrap().to_owned();
            app.load_folder(parent.clone());
            settle(&mut app);
            if entrance == "history" {
                add_history(&app, &source, history_kind);
                enter(&mut app, Entrance::Menu);
            } else {
                app.open_favsearch();
                app.favsearch.results_paths = vec![source.clone()];
                app.install_new_items(
                    vec![match kind {
                        "folder" | "image_book" => GridItem::Folder(source.clone()),
                        "zip" => GridItem::ZipFile(source.clone()),
                        "pdf" | "epub" => GridItem::PdfFile(source.clone()),
                        _ => GridItem::ConvertibleArchive {
                            path: source.clone(),
                            format: crate::archive_converter::ArchiveFormat::Rar,
                        },
                    }],
                    vec![None],
                );
                app.visible_indices = vec![0];
            }
            open_container_path(&mut app, &source);
            if entrance == "global" {
                // Adopted physical source opened from a Ctrl+G result. Search result creation
                // is tested elsewhere; this tests its physical reader owner and explicit exit.
                app.favsearch = Default::default();
                app.global_search.active = true;
                app.global_search.drill = Some(crate::global_search_ui::DrillState {
                    container_root: source.clone(),
                    current_path: source.clone(),
                    is_zip: kind == "zip",
                });
            }
            assert!(
                !matches!(app.grid_parent_action(), GridParentAction::Navigation { nav: AddressBarNav::Direct(ref p, _), .. } if p == &parent),
                "{entrance}/{kind}"
            );
            if kind == "zip" {
                app.zip_nav_enter("chapter/inner.zip/deep/");
                app.finish_page_identity_preparation_for_test();
                settle(&mut app);
                assert_eq!(
                    app.zip_nav.as_ref().unwrap().current().join("/"),
                    "chapter/inner.zip/deep"
                );
            }
            assert_jump(&app, &source);
            app.handle_grid_physical_parent_action();
            finish_jump(&mut app);
            assert_eq!(
                app.current_folder.as_ref(),
                Some(&parent),
                "{entrance}/{kind}"
            );
            assert!(
                app.selected.is_some_and(
                    |index| app.items[index].container_path() == Some(source.as_path())
                ),
                "{entrance}/{kind}"
            );
            assert!(app.reading_history_return_from.is_none());
            assert!(!app.global_search.active && !app.favsearch.active);
            assert!(
                matches!(app.grid_parent_action(), GridParentAction::Navigation { nav: AddressBarNav::Direct(ref p, _), .. } if Some(p.as_path()) == parent.parent()),
                "{entrance}/{kind}"
            );
        }
    }
}

#[test]
fn section1672_history_descendant_jump_does_not_resurrect_expired_return() {
    let mut app = setup(false);
    configure(&mut app);
    let (source, history_kind) = fixture(&mut app, "folder");
    app.load_folder(source.parent().unwrap().to_owned());
    settle(&mut app);
    add_history(&app, &source, history_kind);
    enter(&mut app, Entrance::Menu);
    open_container_path(&mut app, &source);
    app.load_folder(source.join("child"));
    settle(&mut app);
    assert!(app.reading_history_return_from.is_none());
    app.handle_grid_physical_parent_action();
    assert!(app.reading_history_return_from.is_none());
    finish_jump(&mut app);
    assert_eq!(app.current_folder.as_ref(), Some(&source));
    assert!(app.reading_history_return_from.is_none());
    assert!(
        matches!(app.grid_parent_action(), GridParentAction::Navigation { nav: AddressBarNav::Direct(ref p, _), .. } if Some(p.as_path()) == source.parent())
    );
}

#[test]
fn section1672_drive_root_explicit_exit_dismisses_search() {
    for search in ["global", "favorite", "tag"] {
        let mut app = setup(false);
        configure(&mut app);
        app.current_folder = Some(PathBuf::from(r"C:\"));
        match search {
            "global" => {
                app.global_search.active = true;
                app.global_search.drill = Some(crate::global_search_ui::DrillState {
                    container_root: PathBuf::from(r"C:\"),
                    current_path: PathBuf::from(r"C:\"),
                    is_zip: false,
                });
            }
            "favorite" => {
                app.favsearch.active = true;
                app.favsearch.nav_stack.push(PathBuf::from(r"C:\"));
            }
            _ => {
                app.tag_view.active = true;
                app.tag_view.nav_stack.push(PathBuf::from(r"C:\"));
            }
        }
        assert_eq!(
            app.grid_physical_parent_action(),
            GridPhysicalParentAction::DriveList(PathBuf::from(r"C:\"))
        );
        app.handle_grid_physical_parent_action();
        settle(&mut app);
        assert!(app.items_are_drive_list, "{search}");
        assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
        assert!(matches!(app.grid_parent_action(), GridParentAction::NoOp));
    }
}
