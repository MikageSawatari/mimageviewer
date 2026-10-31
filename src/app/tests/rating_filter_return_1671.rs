//! §1.671: compare logical/book backing pairs through real admission, adoption,
//! close and parent navigation. No product window or native input is used.
use super::*;

const FIVE: [bool; 6] = [false, false, false, false, false, true];

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
    } else {
        assert!(app.archive_source_override.is_none());
        assert_eq!(app.current_folder, Some(root.clone()));
        assert_eq!(app.effective_folder(), Some(root.clone()));
        assert_eq!(
            app.rating_filter_suppressed_at.as_ref().map(|(p, _)| p),
            parent_starred.then_some(&root),
            "old book anchor must leave scope; a starred parent establishes its own anchor"
        );
        assert_eq!(
            app.effective_rating_filter(),
            if parent_starred { [true; 6] } else { FIVE }
        );
        if parent_starred {
            assert_eq!(visible(&app), app.items);
            assert!(visible(&app).contains(&unstarred));
        } else {
            assert_eq!(visible(&app), vec![row]);
        }
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
