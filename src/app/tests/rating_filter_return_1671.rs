//! §1.671: compare logical/book backing pairs through real admission, adoption,
//! close and parent navigation. No product window or native input is used.
use super::*;
#[path = "rating_filter_parent_return_1721.rs"]
mod rating1721;

const FIVE: [bool; 6] = [false, false, false, false, false, true];

fn grid_key(app: &mut App, key: egui::Key, modifiers: egui::Modifiers) {
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        events: vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            },
        ],
        ..Default::default()
    });
    assert!(
        app.handle_keyboard(&ctx).is_none(),
        "ZIP hierarchy navigation is consumed by its owner"
    );
    ctx.end_pass().drop_without_applying_deltas();
}

fn zip_level_return(converted: bool, nested: bool, everything: bool, dfs: bool) {
    use std::io::Write as _;
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.active_quick_folder_slot = None;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.auto_fullscreen_zip_pdf = false;
    app.settings.sort_order = crate::settings::SortOrder::FileName;
    let root = app.tmp.path().join("zip-level-return");
    std::fs::create_dir(&root).unwrap();
    let (row, backing) = fixture(
        &mut app,
        &root,
        if converted {
            Book::Converted
        } else {
            Book::Zip
        },
    );
    let source = row.drag_source_path().unwrap().to_path_buf();
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(2, 2)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let mut inner = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    inner
        .start_file("001.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    inner.write_all(png.get_ref()).unwrap();
    let inner = inner.finish().unwrap().into_inner();
    let mut outer = zip::ZipWriter::new(std::fs::File::create(&backing).unwrap());
    for name in ["A", "B"] {
        outer
            .start_file(
                if nested {
                    format!("{name}.zip")
                } else {
                    format!("{name}/001.png")
                },
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        outer
            .write_all(if nested { &inner } else { png.get_ref() })
            .unwrap();
    }
    outer.finish().unwrap();
    if converted {
        let stamp = std::fs::metadata(&source).unwrap();
        app.archive_cache_db
            .as_ref()
            .unwrap()
            .record(
                &source,
                crate::ui_helpers::mtime_secs(&stamp),
                stamp.len() as i64,
                crate::archive_converter::ArchiveFormat::SevenZ,
                &backing,
                std::fs::metadata(&backing).unwrap().len() as i64,
                2,
                false,
            )
            .unwrap();
    }
    let a = if nested { "A.zip" } else { "A" };
    app.rating_db
        .as_ref()
        .unwrap()
        .set(&crate::adjustment_db::normalize_path(&source.join(a)), 5)
        .unwrap();
    app.settings.rating_filter = FIVE;
    if everything {
        app.rating_db
            .as_ref()
            .unwrap()
            .set(&crate::adjustment_db::normalize_path(&source), 5)
            .unwrap();
        adopt(&mut app, &ctx, "outer book", dataset(vec![row.clone()]));
        open(&mut app, &ctx, &row, false);
        assert!(
            app.restore_rating_filter_suppression(),
            "same action as the suppression badge"
        );
        app.rebuild_visible_indices();
    } else {
        let outcome = app.load_folder_or_convert_archive_with_auto_fullscreen_owned(
            source.clone(),
            false,
            OpenRequestOwner::Navigation,
            StartupListIntent::ExplicitList,
        );
        assert!(matches!(
            outcome,
            FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
        ));
        settle(&mut app, &ctx, None);
    }
    let a_row = app
        .items
        .iter()
        .find(|item| {
            matches!(item,
        GridItem::ZipDir { dir_prefix, .. } if dir_prefix == &format!("{a}/"))
        })
        .unwrap()
        .clone();
    assert_eq!(app.items.len(), 2, "both rated A and unrated B exist");
    assert_eq!(visible(&app), vec![a_row.clone()]);
    trace(&app, "ZIP parent filtered");
    app.selected = app.items.iter().position(|item| item == &a_row);
    grid_key(&mut app, egui::Key::Enter, egui::Modifiers::NONE);
    settle(&mut app, &ctx, None);
    if everything {
        // Existing boundary: Search has no folder-history current entry, so this
        // child-level request does not install its rows. Do not claim a successful
        // nested-book round trip or patch that separate adoption owner here.
        trace(&app, "Everything results control after level request");
        assert!(app.apply_fullscreen_close_nav_immediate(
            crate::ui_main::AddressBarNav::EverythingResults
        ));
        settle(&mut app, &ctx, None);
        assert!(matches!(
            &app.global_search.accepted,
            SearchAccepted::Everything {
                position: EverythingPosition::Root,
                ..
            }
        ));
        assert!(app.rating_filter_suppressed_at.is_none());
        assert_eq!(app.effective_rating_filter(), FIVE);
        assert_eq!(visible(&app), vec![row]);
        return;
    }
    assert_eq!(
        app.rating_filter_suppressed_at.as_ref().map(|(p, _)| p),
        Some(&source.join(a))
    );
    assert_eq!(app.effective_rating_filter(), [true; 6]);
    assert_eq!(visible(&app), app.items);
    assert_eq!(app.items.len(), 1);
    app.selected = Some(0);
    grid_key(&mut app, egui::Key::Enter, egui::Modifiers::NONE);
    settle(&mut app, &ctx, None);
    assert!(app.fullscreen_idx.is_some());
    key(&mut app, &ctx, egui::Key::Backspace);
    assert!(app.fullscreen_idx.is_none());
    assert!(
        app.rating_filter_suppressed_at.is_some(),
        "page list stays inside A"
    );
    assert_eq!(visible(&app), app.items);
    if dfs {
        grid_key(&mut app, egui::Key::ArrowUp, egui::Modifiers::CTRL);
    } else {
        grid_key(&mut app, egui::Key::Backspace, egui::Modifiers::NONE);
    }
    settle(&mut app, &ctx, None);
    trace(
        &app,
        &format!(
            "ZIP parent returned converted={converted} nested={nested} everything={everything} dfs={dfs}"
        ),
    );
    assert!(app.zip_nav.as_ref().unwrap().at_root());
    assert_eq!(app.effective_folder(), Some(source));
    assert_eq!(app.current_folder, Some(backing));
    assert_eq!(app.items.len(), 2, "B remains in the source rows");
    assert_eq!(
        visible(&app),
        vec![a_row],
        "unrated B must not remain visible after leaving A"
    );
    assert!(app.rating_filter_suppressed_at.is_none());
    assert_eq!(app.effective_rating_filter(), FIVE);
    assert_eq!(app.settings.rating_filter, FIVE);
}

macro_rules! zip_level_cases {
    ($name:ident, $converted:expr, $nested:expr) => {
        mod $name {
            use super::*;
            #[test]
            fn backspace() {
                zip_level_return($converted, $nested, false, false);
            }
            #[test]
            fn grid_dfs_back() {
                zip_level_return($converted, $nested, false, true);
            }
            #[test]
            fn everything_results_after_level_request() {
                zip_level_return($converted, $nested, true, false);
            }
        }
    };
}
zip_level_cases!(zip_level_directory, false, false);
zip_level_cases!(zip_level_nested_archive, false, true);
zip_level_cases!(converted_zip_level_directory, true, false);
zip_level_cases!(converted_zip_level_nested_archive, true, true);

#[derive(Clone, Copy, Debug)]
enum Return {
    Escape,
    Hud,
    PageList,
}

fn visible(app: &App) -> Vec<GridItem> {
    app.current_grid_order()
        .iter()
        .map(|&i| app.items[i].clone())
        .collect()
}

fn trace(app: &App, stage: &str) {
    eprintln!(
        "1671 {stage}: logical={:?} backing={:?} override={:?} anchor={:?} effective={:?} visible={:?}",
        app.effective_folder(),
        app.current_folder,
        app.archive_source_override,
        app.rating_filter_suppressed_at,
        app.effective_rating_filter(),
        visible(app)
    );
    assert_eq!(app.settings.rating_filter, FIVE);
}

fn settle(app: &mut App, ctx: &egui::Context, pdf: Option<&Path>) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut injected = false;
    loop {
        app.settle_open_path_classification_for_test();
        // Only PDF enumeration is faked; admission, preparation, ownership and
        // visible install remain production code, as in the parent test module.
        if !injected
            && let Some(path) = pdf
            && let Some(pending) = app.pdf_enumerate_pending.as_mut()
        {
            pending.2 = crate::pdf_loader::completed_enumerate_result_handle(
                path,
                Ok(vec![crate::pdf_loader::PdfPageEntry {
                    page_num: 0,
                    mtime: 1,
                    file_size: 1,
                }]
                .into()),
            );
            injected = true;
        }
        app.poll_history_navigation_transition_for_test(ctx);
        app.poll_collection_history_transition(ctx);
        app.poll_zip_enumerate();
        app.poll_pdf_enumerate();
        app.poll_search_page_edit_prepare(ctx);
        app.poll_sidecar_restore(ctx);
        app.poll_book_composition_service_for_test(ctx);
        app.poll_book_presentation_permission(ctx);
        if app.top_level_grid_view.open_path_classification().is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && app.zip_enumerate_pending.is_none()
            && app.pdf_enumerate_pending.is_none()
            && app.global_search.pending.is_none()
            && !app.sidecar_restore_active()
        {
            app.finish_page_identity_preparation_for_test();
            return;
        }
        assert!(Instant::now() < deadline, "1671 adoption did not settle");
        std::thread::yield_now();
    }
}

fn admit(app: &mut App, ctx: &egui::Context, row: &GridItem) {
    let index = app.items.iter().position(|item| item == row).unwrap();
    app.selected = Some(index);
    if let GridItem::ConvertibleArchive { path, .. } = row {
        let owner = app.main_grid_archive_open_owner(index, path);
        let outcome = app.load_folder_or_convert_archive_with_auto_fullscreen_owned(
            path.clone(),
            true,
            owner,
            StartupListIntent::container_open(true),
        );
        assert!(matches!(
            outcome,
            FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
        ));
    } else {
        let nav = app.accept_grid_container_open(ctx, index).unwrap();
        assert!(app.apply_fullscreen_close_nav_immediate(nav));
    }
    let pdf = match row {
        GridItem::PdfFile(path) => Some(path.as_path()),
        _ => None,
    };
    settle(app, ctx, pdf);
    app.reopen_fullscreen_after_folder_nav_load(ctx, false, false, HistoryTrigger::UserChosen);
    assert!(
        app.fullscreen_idx.is_some(),
        "book must open its unrated page"
    );
}

fn link(app: &mut App, ctx: &egui::Context) {
    key(app, ctx, egui::Key::F12);
    assert_eq!(app.viewer_presentation, ViewerPresentation::DetachedWindow);
    assert!(!app.detached_viewer_independent_active);
    let session = app.active_detached_session.unwrap();
    assert_eq!(
        app.locate_window_context(session.window_id),
        Some((app.viewer_context_main(), ContextResidence::Mounted))
    );
    let viewport_id = app.fullscreen_viewport_id();
    ctx.begin_pass(egui::RawInput {
        viewport_id,
        viewports: [
            (egui::ViewportId::ROOT, egui::ViewportInfo::default()),
            (
                viewport_id,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    ..Default::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    });
    app.mark_active_detached_viewport_rendered(ctx);
    app.fs_viewport_shown = true;
    app.fs_viewport_presentation = Some(ViewerPresentation::DetachedWindow);
    ctx.end_pass().drop_without_applying_deltas();
}

fn scenario(book: Book, linked: bool, parent_starred: bool, everything: bool, route: Return) {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.active_quick_folder_slot = None;
    app.settings.auto_fullscreen_zip_pdf = true;
    app.settings.auto_fullscreen_image_folders = true;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.detached_viewer_enabled = false;
    app.settings.sort_order = crate::settings::SortOrder::FileName;
    let root = app.tmp.path().join("shelf");
    std::fs::create_dir(&root).unwrap();
    let (row, backing) = fixture(&mut app, &root, book);
    let source = row.drag_source_path().unwrap().to_path_buf();
    let unstarred = GridItem::Image(root.join("unstarred.png"));
    image::RgbImage::new(2, 2)
        .save(unstarred.drag_source_path().unwrap())
        .unwrap();
    app.rating_db
        .as_ref()
        .unwrap()
        .set(&crate::adjustment_db::normalize_path(&source), 5)
        .unwrap();
    app.settings.rating_filter = FIVE;
    if everything {
        adopt(
            &mut app,
            &ctx,
            "book",
            dataset(vec![row.clone(), unstarred.clone()]),
        );
    } else {
        app.load_folder(root.clone());
    }
    app.finish_page_identity_preparation_for_test();
    assert_eq!(visible(&app), vec![row.clone()]);
    // A starred parent can have an explicitly active filter (badge restore).
    // Set its rating after initial adoption to keep the outgoing list identical.
    if parent_starred {
        app.rating_db
            .as_ref()
            .unwrap()
            .set(&crate::adjustment_db::normalize_path(&root), 5)
            .unwrap();
    }
    let sibling_anchor = app.tmp.path().join("independent");
    let sibling = app.build_window_context_for_test(1671, |other| {
        other.current_folder = Some(sibling_anchor.clone());
        other.rating_filter_suppressed_at = Some((sibling_anchor.clone(), FIVE));
    });
    let sibling_before = app
        .with_viewer_context(sibling, |other| {
            (
                other.current_folder.clone(),
                other.archive_source_override.clone(),
                other.rating_filter_suppressed_at.clone(),
                other.items.clone(),
                other.items_generation,
                other.pdf_render_epoch.snapshot(),
            )
        })
        .unwrap();
    trace(
        &app,
        &format!(
            "parent {book:?} linked={linked} starred={parent_starred} everything={everything} route={route:?}"
        ),
    );
    admit(&mut app, &ctx, &row);
    trace(&app, "book adopted");
    assert_eq!(app.effective_folder(), Some(source.clone()));
    assert_eq!(app.current_folder, Some(backing.clone()));
    assert_eq!(
        app.archive_source_override.as_ref(),
        if matches!(book, Book::Converted) {
            Some(&source)
        } else {
            None
        }
    );
    assert_eq!(
        app.rating_filter_suppressed_at.as_ref().map(|(p, _)| p),
        Some(&source)
    );
    assert_eq!(app.effective_rating_filter(), [true; 6]);
    assert_eq!(visible(&app), app.items);
    assert_eq!(app.items.len(), 1);
    assert_eq!(app.get_rating(0), 0);
    if linked {
        link(&mut app, &ctx);
    }
    let close = |viewer: &mut App| {
        match route {
            Return::Escape => viewer_key(viewer, &ctx, egui::Key::Escape),
            // HUD × uses the same request consumer, without a synthetic key.
            Return::Hud => viewer.handle_fullscreen_close_request(),
            Return::PageList => viewer_key(viewer, &ctx, egui::Key::Backspace),
        }
    };
    if linked {
        app.with_active_viewer_context(close)
            .expect("linked context");
    } else {
        close(&mut app);
    }
    if matches!(route, Return::PageList) {
        trace(&app, "page list");
        assert!(app.fullscreen_idx.is_none());
        assert_eq!(app.current_folder, Some(backing.clone()));
        assert_eq!(app.effective_folder(), Some(source.clone()));
        assert_eq!(
            app.rating_filter_suppressed_at.as_ref().map(|(p, _)| p),
            Some(&source)
        );
        assert_eq!(app.effective_rating_filter(), [true; 6]);
        assert_eq!(visible(&app), app.items);
        assert!(!app.pending_return_to_parent.is_pending());
        // Everything consumes its typed return itself, while a physical parent
        // yields navigation for the main-frame dispatcher.
        if let Some(nav) = app.handle_grid_parent_folder_action() {
            assert!(app.apply_fullscreen_close_nav_immediate(nav));
        } else {
            assert!(everything && app.global_search.pending.is_some());
        }
    } else {
        let nav = app
            .take_pending_return_to_parent_nav()
            .expect("normal close return");
        assert!(app.apply_fullscreen_close_nav_immediate(nav));
    }
    settle(&mut app, &ctx, None);
    trace(&app, "returned parent/results");
    assert!(app.fullscreen_idx.is_none());
    if everything {
        assert!(!app.everything_has_physical_child());
        // Search owns a synthetic surface. The existing result install retains
        // the last physical backing/alias; accepted EverythingPosition owns the
        // logical location instead of effective_folder in this surface.
        assert!(matches!(
            &app.global_search.accepted,
            SearchAccepted::Everything {
                position: EverythingPosition::Root,
                ..
            }
        ));
        assert_eq!(app.current_folder.as_ref(), Some(&backing));
        assert_eq!(
            app.archive_source_override.as_ref(),
            if matches!(book, Book::Converted) {
                Some(&source)
            } else {
                None
            }
        );
        assert!(app.rating_filter_suppressed_at.is_none());
        assert_eq!(app.effective_rating_filter(), FIVE);
        assert_eq!(visible(&app), vec![row]);
        assert!(!app.rating_filter_departure_is_physical());
        // Address/navigation admission must reopen the same last book as a new
        // entry, without the grid-open helper pre-suppressing its rating filter.
        let outcome = app.load_folder_or_convert_archive_with_auto_fullscreen_owned(
            source.clone(),
            false,
            OpenRequestOwner::Navigation,
            StartupListIntent::ExplicitList,
        );
        assert!(matches!(
            outcome,
            FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
        ));
        settle(
            &mut app,
            &ctx,
            matches!(book, Book::Pdf).then_some(source.as_path()),
        );
        assert_eq!(app.effective_folder(), Some(source.clone()));
        assert_eq!(
            app.rating_filter_suppressed_at.as_ref().map(|(p, _)| p),
            Some(&source)
        );
        assert_eq!(app.effective_rating_filter(), [true; 6]);
        assert_eq!(app.items.len(), 1);
        assert_eq!(visible(&app), app.items);
        assert_eq!(app.get_rating(0), 0);
        assert!(app.everything_has_physical_child());
        assert!(app.rating_filter_departure_is_physical());
        // PhysicalChild is a real location: an explicitly restored filter must
        // remain active on its same-book reload, unlike results-root reentry.
        assert!(app.restore_rating_filter_suppression());
        app.rebuild_visible_indices();
        let before_reload = app.items_generation;
        app.reload_current_folder_preserving_override();
        settle(
            &mut app,
            &ctx,
            matches!(book, Book::Pdf).then_some(source.as_path()),
        );
        assert!(app.rating_filter_suppressed_at.is_none());
        assert_eq!(app.effective_rating_filter(), FIVE);
        assert_eq!(app.items.len(), 1);
        assert!(visible(&app).is_empty());
        assert_ne!(
            app.items_generation, before_reload,
            "reload must adopt rows"
        );
    } else {
        assert!(app.archive_source_override.is_none());
        assert_eq!(app.current_folder, Some(root.clone()));
        assert_eq!(app.effective_folder(), Some(root.clone()));
        assert!(
            app.rating_filter_suppressed_at.is_none(),
            "returning to the list must not establish a new anchor from the parent's stars"
        );
        assert_eq!(app.effective_rating_filter(), FIVE);
        assert_eq!(visible(&app), vec![row]);
        assert!(!visible(&app).contains(&unstarred));
    }
    app.with_viewer_context(sibling, |other| {
        assert_eq!(other.current_folder, sibling_before.0);
        assert_eq!(other.archive_source_override, sibling_before.1);
        assert_eq!(other.rating_filter_suppressed_at, sibling_before.2);
        assert_eq!(other.items, sibling_before.3);
        assert_eq!(other.items_generation, sibling_before.4);
        assert!(sibling_before.5.is_current());
        assert_eq!(other.effective_rating_filter(), [true; 6]);
    })
    .unwrap();
    app.settings.save();
    assert_eq!(crate::settings::Settings::load().rating_filter, FIVE);
}

macro_rules! cases {
    ($name:ident, $book:ident, $linked:expr, $starred:expr) => {
        mod $name {
            use super::*;
            macro_rules! origin {
                ($origin:ident, $everything:expr) => {
                    mod $origin {
                        use super::*;
                        #[test]
                        fn escape() {
                            scenario(Book::$book, $linked, $starred, $everything, Return::Escape);
                        }
                        #[test]
                        fn hud() {
                            scenario(Book::$book, $linked, $starred, $everything, Return::Hud);
                        }
                        #[test]
                        fn page_list_then_parent() {
                            scenario(
                                Book::$book,
                                $linked,
                                $starred,
                                $everything,
                                Return::PageList,
                            );
                        }
                    }
                };
            }
            origin!(folder, false);
            origin!(everything, true);
        }
    };
}
cases!(zip_main, Zip, false, false);
cases!(zip_main_starred_parent, Zip, false, true);
cases!(zip_linked, Zip, true, false);
cases!(zip_linked_starred_parent, Zip, true, true);
cases!(converted_main, Converted, false, false);
cases!(converted_main_starred_parent, Converted, false, true);
cases!(converted_linked, Converted, true, false);
cases!(converted_linked_starred_parent, Converted, true, true);
cases!(pdf_main, Pdf, false, false);
cases!(pdf_main_starred_parent, Pdf, false, true);
cases!(pdf_linked, Pdf, true, false);
cases!(pdf_linked_starred_parent, Pdf, true, true);
cases!(images_main, Folder, false, false);
cases!(images_main_starred_parent, Folder, false, true);
cases!(images_linked, Folder, true, false);
cases!(images_linked_starred_parent, Folder, true, true);

#[test]
fn same_backing_reload_and_switch_use_the_adopted_source() {
    for book in [Book::Zip, Book::Converted, Book::Pdf, Book::Folder] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        app.active_quick_folder_slot = None;
        app.settings.auto_fullscreen_zip_pdf = true;
        app.settings.auto_fullscreen_image_folders = true;
        app.settings.detached_viewer_open_images_in_window = false;
        let root = app.tmp.path().join("source");
        std::fs::create_dir(&root).unwrap();
        let (row, backing) = fixture(&mut app, &root, Book::Converted);
        let source = row.drag_source_path().unwrap().to_path_buf();
        app.rating_db
            .as_ref()
            .unwrap()
            .set(&crate::adjustment_db::normalize_path(&source), 5)
            .unwrap();
        app.settings.rating_filter = FIVE;
        app.load_folder(root);
        admit(&mut app, &ctx, &row);
        app.close_fullscreen();
        // F5's same-backing load must retain the original archive identity.
        app.load_folder(backing.clone());
        settle(&mut app, &ctx, None);
        assert_eq!(app.current_folder, Some(backing));
        assert_eq!(app.effective_folder(), Some(source.clone()));
        assert_eq!(
            app.rating_filter_suppressed_at.as_ref().map(|(p, _)| p),
            Some(&source)
        );
        let destination = app.tmp.path().join("destination");
        std::fs::create_dir(&destination).unwrap();
        let (next, next_backing) = fixture(&mut app, &destination, book);
        let next_source = next.drag_source_path().unwrap().to_path_buf();
        app.rating_db
            .as_ref()
            .unwrap()
            .set(&crate::adjustment_db::normalize_path(&next_source), 5)
            .unwrap();
        let outcome = app.load_folder_or_convert_archive_with_auto_fullscreen_owned(
            next_source.clone(),
            false,
            OpenRequestOwner::Navigation,
            StartupListIntent::ExplicitList,
        );
        assert!(matches!(
            outcome,
            FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
        ));
        settle(
            &mut app,
            &ctx,
            matches!(book, Book::Pdf).then_some(next_source.as_path()),
        );
        trace(&app, &format!("switch from converted to {book:?}"));
        assert_eq!(app.current_folder, Some(next_backing));
        assert_eq!(app.effective_folder(), Some(next_source.clone()));
        assert_eq!(
            app.rating_filter_suppressed_at.as_ref().map(|(p, _)| p),
            Some(&next_source)
        );
        assert_eq!(app.effective_rating_filter(), [true; 6]);
        assert_eq!(visible(&app), app.items);
    }
}

#[test]
fn unadopted_canceled_and_failed_pdf_leave_the_converted_book_unchanged() {
    for cancel in [true, false] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        app.active_quick_folder_slot = None;
        app.settings.auto_fullscreen_zip_pdf = true;
        app.settings.detached_viewer_open_images_in_window = false;
        let root = app.tmp.path().join("source");
        std::fs::create_dir(&root).unwrap();
        let (row, _) = fixture(&mut app, &root, Book::Converted);
        let source = row.drag_source_path().unwrap().to_path_buf();
        app.rating_db
            .as_ref()
            .unwrap()
            .set(&crate::adjustment_db::normalize_path(&source), 5)
            .unwrap();
        app.settings.rating_filter = FIVE;
        app.load_folder(root.clone());
        admit(&mut app, &ctx, &row);
        app.close_fullscreen();
        let before = (
            app.current_folder.clone(),
            app.archive_source_override.clone(),
            app.rating_filter_suppressed_at.clone(),
            app.items.clone(),
            app.items_generation,
            visible(&app),
        );
        let failed = root.join("failed-open");
        std::fs::create_dir(&failed).unwrap();
        let (_, pdf) = fixture(&mut app, &failed, Book::Pdf);
        assert_eq!(
            app.load_pdf_as_folder_owned(
                pdf.clone(),
                OpenRequestOwner::Navigation,
                StartupListIntent::ExplicitList
            ),
            FolderOpenOutcome::Loaded
        );
        if cancel {
            app.pdf_enumerate_pending
                .as_ref()
                .unwrap()
                .2
                .cancel
                .store(true, Ordering::Relaxed);
        } else {
            app.pdf_enumerate_pending.as_mut().unwrap().2 =
                crate::pdf_loader::completed_enumerate_result_handle(
                    &pdf,
                    Err(std::io::Error::other("1671 enumeration failure")),
                );
        }
        app.poll_pdf_enumerate();
        assert!(
            app.pdf_enumerate_pending.is_none(),
            "terminal result must be consumed"
        );
        assert_eq!(
            (
                app.current_folder.clone(),
                app.archive_source_override.clone(),
                app.rating_filter_suppressed_at.clone(),
                app.items.clone(),
                app.items_generation,
                visible(&app)
            ),
            before
        );
        trace(
            &app,
            if cancel {
                "PDF canceled before adoption"
            } else {
                "PDF failed before adoption"
            },
        );
    }
}

fn unadopted_pdf_password_terminal(pause: bool) {
    for book in [Book::Zip, Book::Converted] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        app.active_quick_folder_slot = None;
        app.settings.auto_fullscreen_zip_pdf = true;
        app.settings.detached_viewer_open_images_in_window = false;
        let root = app.tmp.path().join("password-source");
        std::fs::create_dir(&root).unwrap();
        let (row, _) = fixture(&mut app, &root, book);
        let source = row.drag_source_path().unwrap().to_path_buf();
        app.rating_db
            .as_ref()
            .unwrap()
            .set(&crate::adjustment_db::normalize_path(&source), 5)
            .unwrap();
        app.settings.rating_filter = FIVE;
        app.load_folder(root.clone());
        admit(&mut app, &ctx, &row);
        app.close_fullscreen();
        let before = (
            app.current_folder.clone(),
            app.archive_source_override.clone(),
            app.rating_filter_suppressed_at.clone(),
            app.items.clone(),
            app.items_generation,
            visible(&app),
            app.effective_rating_filter(),
        );
        let pdf = root.join("password.pdf");
        std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
        assert_eq!(
            app.load_pdf_as_folder_owned(
                pdf.clone(),
                OpenRequestOwner::Navigation,
                StartupListIntent::ExplicitList
            ),
            FolderOpenOutcome::Loaded
        );
        app.pdf_enumerate_pending.as_mut().unwrap().2 =
            crate::pdf_loader::completed_enumerate_result_handle(
                &pdf,
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    crate::pdf_loader::PdfReadError::PasswordRequired,
                )),
            );
        app.poll_pdf_enumerate();
        assert!(app.pdf_enumerate_pending.is_none());
        assert!(matches!(
            app.pdf_password_request.as_ref().map(|r| &r.owner),
            Some(PdfPasswordRequestOwner::Direct(_))
        ));
        assert_eq!(
            app.rating_filter_suppressed_at, before.2,
            "prompt has not adopted the PDF"
        );
        if pause {
            // Shared terminal used when the owning context is parked; no viewport
            // or native window is created by this owner-level regression.
            assert!(app.finish_pdf_password_request_in_mounted_context(PdfPasswordExit::Parked));
        } else {
            assert!(app.cancel_pdf_password_dialog_request());
        }
        assert!(app.pdf_password_request.is_none());
        trace(
            &app,
            if pause {
                "unadopted password parked"
            } else {
                "unadopted password canceled"
            },
        );
        assert_eq!(
            (
                app.current_folder.clone(),
                app.archive_source_override.clone(),
                app.rating_filter_suppressed_at.clone(),
                app.items.clone(),
                app.items_generation,
                visible(&app),
                app.effective_rating_filter()
            ),
            before,
            "unadopted password terminal must preserve the source filter and visible rows"
        );
    }
}

#[test]
fn unadopted_pdf_password_cancel_preserves_source_suppression_and_visible_rows() {
    unadopted_pdf_password_terminal(false);
}

#[test]
fn unadopted_pdf_password_park_preserves_source_suppression_and_visible_rows() {
    unadopted_pdf_password_terminal(true);
}
