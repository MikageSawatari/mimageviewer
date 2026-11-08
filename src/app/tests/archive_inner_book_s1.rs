//! S1 handler/lifecycle regressions. Real ZIP workers, isolated data, no OS input.
use super::*;
use crate::ui_main::AddressBarNav;
use std::io::Write;

fn png() -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(8, 12)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn zip_bytes(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn write_pages(path: &Path, names: &[&str]) {
    let entries = names.iter().map(|name| (*name, png())).collect::<Vec<_>>();
    std::fs::write(path, zip_bytes(&entries)).unwrap();
}

fn settle(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_open_path_classification(ctx);
        app.poll_collection_history_transition(ctx);
        app.poll_zip_enumerate();
        app.poll_sidecar_restore(ctx);
        app.poll_book_composition_service_for_test(ctx);
        app.poll_fs_nav_lock(ctx);
        if app.zip_enumerate_pending.is_none()
            && app.top_level_grid_view.open_path_classification().is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && !app.sidecar_restore_active()
            && !app.book_composition.is_pending()
            && !app.book_composition_permission.pending()
            && !app.fs_nav_deferred_reopen_wait_active()
        {
            break;
        }
        assert!(Instant::now() < deadline, "archive open did not settle");
        std::thread::yield_now();
    }
}

fn open_list(app: &mut App, ctx: &egui::Context, path: &Path) {
    app.indexer_init = IndexerInit::Unavailable;
    app.settings.detached_viewer_open_images_in_window = false;
    assert!(matches!(
        app.load_folder_or_convert_archive_with_auto_fullscreen(path.to_owned(), false),
        FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
    ));
    settle(app, ctx);
    assert!(app.fullscreen_idx.is_none());
    assert!(app.zip_nav.as_ref().unwrap().at_root());
}

fn child(app: &App, prefix: &str) -> usize {
    app.items
        .iter()
        .position(
            |item| matches!(item, GridItem::ZipDir { dir_prefix, .. } if dir_prefix == prefix),
        )
        .unwrap_or_else(|| panic!("missing {prefix}: {:?}", app.items))
}

fn activate(app: &mut App, ctx: &egui::Context, prefix: &str, page: bool) {
    let index = child(app, prefix);
    app.selected = Some(index);
    let result = app.activate_main_zipdir(index);
    if page {
        assert!(matches!(result, ZipDirActivationOutcome::BookRequested));
    } else {
        assert!(matches!(result, ZipDirActivationOutcome::List));
    }
    settle(app, ctx);
}

fn assert_returned(app: &App, prefix: &[&str], selected_prefix: &str) {
    assert!(app.fullscreen_idx.is_none());
    assert_eq!(app.zip_nav.as_ref().unwrap().current(), prefix);
    assert_eq!(app.selected, Some(child(app, selected_prefix)));
    assert!(!app.pending_return_to_parent.is_pending());
}

fn consume_close(app: &mut App) {
    let nav = app
        .take_pending_return_to_parent_nav()
        .expect("close recipe");
    assert!(matches!(nav, AddressBarNav::ZipListReturn(_)));
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    assert!(app.take_pending_return_to_parent_nav().is_none());
}

#[test]
fn section1675_zipdir_setting_matrix() {
    for nested in [false, true] {
        for auto in [false, true] {
            for image_folder_books in [false, true] {
                let mut app = setup_app_for_test();
                let ctx = egui::Context::default();
                let book = app.tmp.path().join("outer.zip");
                let prefix = if nested { "A.zip/" } else { "A/" };
                if nested {
                    let inner = zip_bytes(&[("001.png", png()), ("002.png", png())]);
                    std::fs::write(
                        &book,
                        zip_bytes(&[("A.zip", inner.clone()), ("B.zip", inner)]),
                    )
                    .unwrap();
                } else {
                    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
                }
                open_list(&mut app, &ctx, &book);
                app.settings.auto_fullscreen_zip_pdf = auto;
                app.settings.auto_fullscreen_image_folders = image_folder_books;
                activate(&mut app, &ctx, prefix, auto);
                assert_eq!(app.fullscreen_idx.is_some(), auto);
                assert_eq!(app.items.len(), 2);
                assert!(
                    app.zip_nav
                        .as_ref()
                        .unwrap()
                        .chosen_list_return_plan()
                        .is_some()
                );
            }
        }
    }
}

#[test]
fn section1675_branch_and_collapsed_wrapper() {
    for wrappers in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("outer.zip");
        write_pages(
            &book,
            if wrappers {
                &["A/one/two/001.png", "A/one/two/002.png", "B/001.png"]
            } else {
                &["A/first/001.png", "A/second/001.png", "B/001.png"]
            },
        );
        open_list(&mut app, &ctx, &book);
        app.settings.auto_fullscreen_zip_pdf = true;
        activate(&mut app, &ctx, "A/", wrappers);
        if wrappers {
            assert_eq!(
                app.zip_nav.as_ref().unwrap().current(),
                &["A", "one", "two"]
            );
            assert!(app.fullscreen_idx.is_some());
            app.handle_fullscreen_close_request_immediate();
            settle(&mut app, &ctx);
            assert_returned(&app, &[], "A/");
        } else {
            assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["A"]);
            assert!(app.fullscreen_idx.is_none());
            assert!(
                app.items
                    .iter()
                    .all(|item| matches!(item, GridItem::ZipDir { .. }))
            );
        }
    }
}

#[test]
fn section1675_s1_ignores_outer_resume_and_does_not_record_inner() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["root.png", "A/001.png", "A/002.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    let identity = crate::book_resume_db::PageIdentity::for_item(
        &GridItem::ZipImage {
            zip_path: book.clone(),
            entry_name: "A/002.png".into(),
        },
        Some(&book),
    )
    .unwrap();
    assert!(app.book_resume_writer.as_ref().unwrap().record_identity(
        &book,
        1,
        None,
        Some(identity)
    ));
    phase_c_support::wait_for_book_resume_write(&app, &book);
    let before = app.book_resume_db.as_ref().unwrap().get(&book);
    assert_eq!(before, Some(1));
    app.settings.book_open_resume = crate::settings::ResumeMode::Resume;
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "A/", true);
    assert_eq!(app.items[app.fullscreen_idx.unwrap()].name(), "001.png");
    app.record_book_resume(1);
    app.handle_fullscreen_close_request_immediate();
    settle(&mut app, &ctx);
    assert_eq!(app.book_resume_db.as_ref().unwrap().get(&book), before);
}

#[test]
fn section1675_cancel_index_preparation_does_not_reopen() {
    for leave_level in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("outer.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
        open_list(&mut app, &ctx, &book);
        app.settings.auto_fullscreen_zip_pdf = true;
        let index = child(&app, "A/");
        assert!(matches!(
            app.activate_main_zipdir(index),
            ZipDirActivationOutcome::BookRequested
        ));
        assert!(
            app.fullscreen_idx.is_none(),
            "identity preparation must precede presentation"
        );
        if leave_level {
            assert!(app.zip_nav_back());
        } else {
            app.handle_fullscreen_close_request();
        }
        settle(&mut app, &ctx);
        assert!(
            app.fullscreen_idx.is_none(),
            "cancelled index completion reopened the book"
        );
    }
}

#[test]
fn section1676_direct_and_manual_open_return() {
    for manual in [false, true] {
        for immediate in [false, true] {
            let mut app = setup_app_for_test();
            let ctx = egui::Context::default();
            let book = app.tmp.path().join("outer.zip");
            write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
            open_list(&mut app, &ctx, &book);
            app.settings.auto_fullscreen_zip_pdf = !manual;
            activate(&mut app, &ctx, "A/", !manual);
            if manual {
                app.open_fullscreen(1, HistoryTrigger::UserChosen);
                app.settings.auto_fullscreen_zip_pdf = true;
            }
            assert!(app.fullscreen_idx.is_some());
            app.spread_popup_open = true;
            if immediate {
                app.handle_fullscreen_close_request_immediate();
            } else {
                app.handle_fullscreen_close_request();
                consume_close(&mut app);
            }
            settle(&mut app, &ctx);
            assert_returned(&app, &[], "A/");
            assert!(
                !app.spread_popup_open,
                "return must run the original fullscreen teardown"
            );
        }
    }
}

#[test]
fn section1676_deep_and_collapsed_return() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    write_pages(
        &book,
        &[
            "shelf/A/only/deep/001.png",
            "shelf/B/001.png",
            "other/001.png",
        ],
    );
    open_list(&mut app, &ctx, &book);
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "shelf/", false);
    activate(&mut app, &ctx, "shelf/A/", true);
    assert_eq!(
        app.zip_nav.as_ref().unwrap().current(),
        &["shelf", "A", "only", "deep"]
    );
    app.handle_fullscreen_close_request_immediate();
    settle(&mut app, &ctx);
    assert_returned(&app, &["shelf"], "shelf/A/");
}

#[test]
fn section1676_dfs_selects_current_sibling() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png", "B/002.png"]);
    open_list(&mut app, &ctx, &book);
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "A/", true);
    assert!(app.zip_nav_dfs_fullscreen(Some(&ctx), 0, true));
    settle(&mut app, &ctx);
    assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["B"]);
    assert!(app.fullscreen_idx.is_some());
    app.handle_fullscreen_close_request_immediate();
    settle(&mut app, &ctx);
    assert_returned(&app, &[], "B/");
}

#[test]
fn section1676_backspace_and_auto_off_keep_l2() {
    for backspace in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("outer.zip");
        write_pages(&book, &["A/001.png", "B/001.png"]);
        open_list(&mut app, &ctx, &book);
        app.settings.auto_fullscreen_zip_pdf = true;
        activate(&mut app, &ctx, "A/", true);
        if backspace {
            app.close_fullscreen_to_page_list();
        } else {
            app.settings.auto_fullscreen_zip_pdf = false;
            app.handle_fullscreen_close_request_immediate();
        }
        settle(&mut app, &ctx);
        assert!(app.fullscreen_idx.is_none());
        assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["A"]);
        assert!(
            app.items
                .iter()
                .all(|item| matches!(item, GridItem::ZipImage { .. }))
        );
    }
}

#[test]
fn section1676_unseen_parent_keeps_old_return() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/001.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    app.zip_nav_enter("A/");
    settle(&mut app, &ctx);
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_none()
    );
    app.settings.auto_fullscreen_zip_pdf = true;
    app.open_fullscreen(0, HistoryTrigger::UserChosen);
    app.handle_fullscreen_close_request();
    let nav = app.take_pending_return_to_parent_nav().unwrap();
    assert!(
        matches!(nav, AddressBarNav::Direct(ref path, _) if Some(path.as_path()) == book.parent())
    );
}

#[test]
fn section1676_scroll_restored_after_layout() {
    for (details, selected) in [(false, 72), (true, 72), (false, 119)] {
        let mut app = setup_app_for_test();
        let book = app.tmp.path().join("outer.zip");
        let names = (0..120)
            .map(|i| format!("book-{i:03}/001.png"))
            .collect::<Vec<_>>();
        write_pages(&book, &names.iter().map(String::as_str).collect::<Vec<_>>());
        let ctx = egui::Context::default();
        open_list(&mut app, &ctx, &book);
        app.settings.grid_cols = 6;
        app.settings.thumb_aspect_auto = false;
        app.settings.grid_view_mode = if details {
            crate::settings::GridViewMode::Details
        } else {
            crate::settings::GridViewMode::Thumbnail
        };
        app.settings.details_sort_key = crate::settings::DetailsSortKey::Name;
        app.settings.details_sort_ascending = true;
        let mut configured = false;
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(1280.0, 1000.0))
            .with_step_dt(0.01)
            .build_ui_state(
                move |ui, app: &mut AppTestEnvForTest| {
                    let ctx = ui.ctx().clone();
                    if !configured {
                        crate::ui_fonts::configure_fonts(&ctx);
                        configured = true;
                        ctx.request_repaint();
                        return;
                    }
                    app.begin_grid_click_input_frame(&ctx);
                    assert!(app.render_grid(ui).is_none());
                },
                app,
            );
        harness.run_steps(6);
        let prefix = format!("book-{selected:03}/");
        let index = child(harness.state(), &prefix);
        let geometry = harness.state().grid_scroll.geometry();
        let position = harness
            .state()
            .current_grid_order()
            .iter()
            .position(|&idx| idx == index)
            .unwrap();
        harness.state_mut().selected = Some(index);
        // Position an actual rendered list as a user scroll would. Geometry is
        // supplied exclusively by render_grid, never by a fixture constructor.
        harness.state_mut().grid_scroll.set_position(
            ((position / geometry.cols).saturating_sub(2) as f32 * geometry.row_h)
                .min(geometry.max_offset),
        );
        harness.run_steps(4);
        let rows_above = harness.state().selected_rows_below_view_top(index);
        if selected == 119 {
            assert!(
                rows_above >= 2,
                "last-row departure must retain the rendered bottom clamp"
            );
        } else {
            assert_eq!(
                rows_above, 2,
                "departure must paint the chosen cell two rows below its top"
            );
        }
        let departure_offset = harness.state().grid_scroll.offset();
        let ctx = harness.ctx.clone();
        {
            let app = harness.state_mut();
            app.settings.auto_fullscreen_zip_pdf = true;
            activate(app, &ctx, &prefix, true);
            if !details {
                app.settings.grid_cols = 8;
            }
            app.handle_fullscreen_close_request_immediate();
            settle(app, &ctx);
            assert_returned(app, &[], &prefix);
            assert_eq!(app.grid_scroll.row_anchor(), Some(rows_above));
            assert!(app.grid_scroll.reveal_requested());
        }
        // Only real destination frames may consume the returned anchor.
        harness.run_steps(4);
        let app = harness.state();
        assert_returned(app, &[], &prefix);
        assert!(app.grid_scroll.row_anchor().is_none());
        assert!(!app.grid_scroll.reveal_requested());
        let geometry = app.grid_scroll.geometry();
        assert_eq!(geometry.cols, if details { 1 } else { 8 });
        let selected = app.selected.unwrap();
        let position = app
            .current_grid_order()
            .iter()
            .position(|&idx| idx == selected)
            .unwrap();
        let expected = ((position / geometry.cols).saturating_sub(rows_above as usize) as f32
            * geometry.row_h)
            .min(geometry.max_offset);
        assert!(
            (app.grid_scroll.offset() - expected).abs() < 0.5,
            "details={details}, selected={prefix}, actual={}, expected={expected}, painted={geometry:?}",
            app.grid_scroll.offset()
        );
        let snapped = app.grid_scroll.offset() / geometry.row_h;
        assert!((snapped - snapped.round()).abs() < 0.01);
        if !details && prefix != "book-119/" {
            assert!(
                (app.grid_scroll.offset() - departure_offset).abs() > geometry.row_h,
                "changed columns must recompute position instead of retaining the old absolute offset"
            );
        }
    }
}

#[test]
fn section1675_zipdir_activation_routes_all_inputs() {
    for nested in [false, true] {
        for input in ["enter", "pad", "double_click"] {
            let mut app = setup_app_for_test();
            let ctx = egui::Context::default();
            let book = app.tmp.path().join("outer.zip");
            let prefix = if nested { "A.zip/" } else { "A/" };
            if nested {
                let inner = zip_bytes(&[("001.png", png()), ("002.png", png())]);
                std::fs::write(
                    &book,
                    zip_bytes(&[("A.zip", inner.clone()), ("B.zip", inner)]),
                )
                .unwrap();
            } else {
                write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
            }
            open_list(&mut app, &ctx, &book);
            app.settings.auto_fullscreen_zip_pdf = true;
            app.selected = Some(child(&app, prefix));
            if input == "double_click" {
                struct State {
                    app: AppTestEnvForTest,
                    origin: egui::Pos2,
                }
                let mut configured = false;
                let mut harness = crate::test_ui::builder()
                    .with_size(egui::vec2(1000.0, 600.0))
                    .with_step_dt(0.01)
                    .build_ui_state(
                        move |ui, state: &mut State| {
                            let ctx = ui.ctx().clone();
                            if !configured {
                                crate::ui_fonts::configure_fonts(&ctx);
                                configured = true;
                                ctx.request_repaint();
                                return;
                            }
                            state.origin = ui.available_rect_before_wrap().min;
                            state.app.begin_grid_click_input_frame(&ctx);
                            assert!(state.app.render_grid(ui).is_none());
                        },
                        State {
                            app,
                            origin: egui::Pos2::ZERO,
                        },
                    );
                harness.run_steps(6);
                let pos = harness.state().origin + egui::vec2(25.0, 25.0);
                harness.hover_at(pos);
                for _ in 0..2 {
                    for pressed in [true, false] {
                        harness.event(egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        });
                    }
                    harness.step();
                }
                let app = &mut harness.state_mut().app;
                settle(app, &ctx);
                assert!(app.fullscreen_idx.is_some(), "{input}/{nested}");
                assert_eq!(
                    app.zip_nav.as_ref().unwrap().current_raw().join("/") + "/",
                    prefix
                );
                assert_eq!(app.items.len(), 2);
                assert!(
                    app.zip_nav
                        .as_ref()
                        .unwrap()
                        .chosen_list_return_plan()
                        .is_some()
                );
            } else {
                if input == "enter" {
                    ctx.begin_pass(egui::RawInput {
                        events: vec![egui::Event::Key {
                            key: egui::Key::Enter,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }],
                        ..Default::default()
                    });
                    assert!(app.handle_keyboard(&ctx).is_none());
                    ctx.end_pass().drop_without_applying_deltas();
                    assert!(app.fs_suppress_enter_close_until_release);
                } else {
                    assert!(app.handle_gamepad_grid_accept(&ctx).is_none());
                }
                settle(&mut app, &ctx);
                assert!(app.fullscreen_idx.is_some(), "{input}/{nested}");
                assert_eq!(
                    app.zip_nav.as_ref().unwrap().current_raw().join("/") + "/",
                    prefix
                );
                assert_eq!(app.items.len(), 2);
                app.handle_fullscreen_close_request_immediate();
                settle(&mut app, &ctx);
                assert_returned(&app, &[], prefix);
            }
        }
    }
}

#[test]
fn section1675_projection_gates_open() {
    use crate::book_composition::ReadingDirection;
    use crate::book_composition::{
        BookComposition, BookCompositionService, CompositionPageRef, PageDomain,
    };
    for (order, hidden, expected) in [
        (
            vec!["002.png", "001.png", "003.png"],
            vec![],
            Some("002.png"),
        ),
        (
            vec!["001.png", "002.png", "003.png"],
            vec!["001.png"],
            Some("002.png"),
        ),
        (
            vec!["001.png", "002.png", "003.png"],
            vec!["001.png", "002.png", "003.png"],
            None,
        ),
    ] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("outer.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "A/003.png", "B/001.png"]);
        let (service, _owner) =
            BookCompositionService::spawn_at(app.tmp.path().join("composition"), None).unwrap();
        let page = |name: &str| CompositionPageRef::ArchiveEntry {
            entry_name: format!("A/{name}"),
        };
        let mut record = BookComposition::new(
            ReadingDirection::Ltr,
            order.iter().map(|name| page(name)).collect(),
        );
        record.hidden_pages = hidden.iter().map(|name| page(name)).collect();
        service
            .save(
                crate::book_scope::BookScopeKey::archive(&book, &["A".into()]).unwrap(),
                PageDomain::ArchiveLevel { prefix: "A".into() },
                record,
            )
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .status
            .unwrap();
        app.install_book_composition_runtime(service);
        open_list(&mut app, &ctx, &book);
        app.settings.auto_fullscreen_zip_pdf = true;
        let index = child(&app, "A/");
        assert!(matches!(
            app.activate_main_zipdir(index),
            ZipDirActivationOutcome::BookRequested
        ));
        assert!(
            app.fullscreen_idx.is_none(),
            "opening must wait for the requested book projection"
        );
        settle(&mut app, &ctx);
        assert_eq!(
            app.fullscreen_idx
                .map(|idx| app.items[idx].name().into_owned())
                .as_deref(),
            expected
        );
        if expected.is_none() {
            assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["A"]);
            assert!(!app.fs_nav_deferred_reopen_wait_active());
        }
    }
}

#[test]
fn section1675_cancel_composition_wait_does_not_reopen() {
    use crate::app::book_composition_runtime::ViewerBookComposition;
    use crate::book_composition::{
        BookComposition, BookCompositionService, CompositionPageRef, PageDomain, ReadingDirection,
    };
    for action in ["close", "level", "archive"] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("outer.zip");
        let replacement = app.tmp.path().join("replacement.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
        write_pages(&replacement, &["C/001.png", "D/001.png"]);
        let (service, _owner) =
            BookCompositionService::spawn_at(app.tmp.path().join("composition"), None).unwrap();
        app.install_book_composition_runtime(service.clone());
        open_list(&mut app, &ctx, &book);
        // An empty scope can be accepted synchronously from the service's
        // immutable snapshot. Keep a real save ahead of Prepare in the worker
        // FIFO so this fixture necessarily exercises a pending composition.
        let (entered, release) = service.hold_worker_for_test();
        entered.recv_timeout(Duration::from_secs(30)).unwrap();
        let record = BookComposition::new(
            ReadingDirection::Ltr,
            ["A/001.png", "A/002.png"]
                .into_iter()
                .map(|entry| CompositionPageRef::ArchiveEntry {
                    entry_name: entry.into(),
                })
                .collect(),
        );
        let saved = service.save(
            crate::book_scope::BookScopeKey::archive(&book, &["A".into()]).unwrap(),
            PageDomain::ArchiveLevel { prefix: "A".into() },
            record,
        );
        app.settings.auto_fullscreen_zip_pdf = true;
        let index = child(&app, "A/");
        assert!(matches!(
            app.activate_main_zipdir(index),
            ZipDirActivationOutcome::BookRequested
        ));
        // Hold the real completion outside its owner, so the absence of a reply
        // cannot depend on worker scheduling. The cancellation precedes delivery.
        app.poll_book_composition_service_for_test(&ctx);
        let ViewerBookComposition::Preparing { receiver, .. } = &mut app.book_composition else {
            // Release the worker before diagnosing an unexpected owner, so a
            // failed assertion cannot leave fixture teardown waiting on it.
            let _ = release.send(());
            panic!(
                "archive scope must wait behind the admitted save: {:?}",
                app.book_composition
            );
        };
        let (deliver, waiting) = mpsc::channel();
        let original = std::mem::replace(receiver, waiting);
        release.send(()).unwrap();
        saved
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .status
            .unwrap();
        let reply = original.recv_timeout(Duration::from_secs(30)).unwrap();
        // IndexOnly's terminal also waits for composition classification, so
        // the general fixture helper must not wait for the whole owner here.
        // Wait for the actual identity-index publication instead, retaining
        // the real after-prepare open intent behind the held composition read.
        let unpublished = Arc::clone(&app.page_identity_index);
        let deadline = Instant::now() + Duration::from_secs(30);
        while Arc::ptr_eq(&unpublished, &app.page_identity_index) {
            app.poll_sidecar_restore(&ctx);
            assert!(
                Instant::now() < deadline,
                "archive identity index was not published"
            );
            std::thread::yield_now();
        }
        assert!(matches!(
            app.book_composition,
            ViewerBookComposition::Preparing { .. }
        ));
        assert!(
            app.sidecar_restore_navigation_fullscreen_wait_active(),
            "the published index retains its open intent while composition is held"
        );
        assert!(app.fullscreen_idx.is_none());
        match action {
            "level" => {
                assert!(app.zip_nav_back());
                app.zip_nav_enter("B/");
            }
            "archive" => {
                // Request admission supersedes the old open; actual source
                // adoption waits for the current IndexOnly owner to finish.
                // Deliver the held classification before waiting for adoption.
                assert!(matches!(
                    app.load_folder_or_convert_archive_with_auto_fullscreen(
                        replacement.clone(),
                        false,
                    ),
                    FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
                ));
            }
            _ => app.handle_fullscreen_close_request(),
        }
        // A replaced scope drops its receiver; a cancelled open may retain the
        // read for list projection. Both must ignore the obsolete open intent.
        eprintln!("composition-cancel: action={action}, delivering held Prepare completion");
        let _ = deliver.send(reply);
        settle(&mut app, &ctx);
        assert!(app.fullscreen_idx.is_none());
        assert!(!app.fs_nav_deferred_reopen_wait_active());
        if action == "archive" {
            assert_eq!(app.current_folder.as_deref(), Some(replacement.as_path()));
            assert!(app.zip_nav.as_ref().unwrap().at_root());
        } else if action == "level" {
            assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["B"]);
        }
    }
}

#[test]
fn section1675_obsolete_index_owner_cannot_replace_another_level_or_archive() {
    for another_archive in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("outer.zip");
        let replacement = app.tmp.path().join("replacement.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
        write_pages(&replacement, &["C/001.png", "D/001.png"]);
        open_list(&mut app, &ctx, &book);
        app.settings.auto_fullscreen_zip_pdf = true;
        let index = child(&app, "A/");
        assert!(matches!(
            app.activate_main_zipdir(index),
            ZipDirActivationOutcome::BookRequested
        ));
        // Retain the actual old owner and worker reply channel outside App,
        // reproducing delivery after a superseding source was fully adopted.
        let obsolete = app
            .sidecar_restore
            .take()
            .expect("accepted identity-index owner");
        if another_archive {
            open_list(&mut app, &ctx, &replacement);
        } else {
            assert!(app.zip_nav_back());
            app.zip_nav_enter("B/");
            settle(&mut app, &ctx);
        }
        let before = (
            app.current_folder.clone(),
            app.items.clone(),
            app.items_generation,
            app.selected,
        );
        assert!(app.sidecar_restore.is_none());
        app.sidecar_restore = Some(obsolete);
        settle(&mut app, &ctx);
        assert!(app.fullscreen_idx.is_none());
        assert!(!app.fs_nav_deferred_reopen_wait_active());
        assert_eq!(
            (
                app.current_folder.clone(),
                app.items.clone(),
                app.items_generation,
                app.selected
            ),
            before
        );
    }
}

#[test]
fn section1675_rating_visibility_selects_first_visible_direct_page() {
    for any_visible in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("outer.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
        if any_visible {
            app.rating_db
                .as_ref()
                .unwrap()
                .set(
                    &crate::edit_source::page_key_for_grid_item(&GridItem::ZipImage {
                        zip_path: book.clone(),
                        entry_name: "A/002.png".into(),
                    })
                    .unwrap(),
                    5,
                )
                .unwrap();
        }
        open_list(&mut app, &ctx, &book);
        app.settings.rating_filter = [false, false, false, false, false, true];
        app.settings.auto_fullscreen_zip_pdf = true;
        activate(&mut app, &ctx, "A/", true);
        assert!(
            app.rating_filter_suppressed_at.is_none(),
            "unrated parent does not suppress the filter"
        );
        assert_eq!(app.fullscreen_idx.is_some(), any_visible);
        if any_visible {
            assert_eq!(app.items[app.fullscreen_idx.unwrap()].name(), "002.png");
        } else {
            assert!(app.visible_indices.is_empty());
            assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["A"]);
        }
    }
}

#[test]
#[cfg(windows)]
fn section1676_terminal_and_live_preserve_bookmark_owner() {
    use crate::bookmark_browser::BookmarkViewReturnTarget;
    for terminal in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("outer.zip");
        write_pages(&book, &["A/001.png", "B/001.png"]);
        open_list(&mut app, &ctx, &book);
        app.bookmark_view_state = Some(BookmarkViewState::Opening {
            target: BookmarkViewReturnTarget::Book(book.clone()),
            grid: BookmarkViewReturnGridState {
                row_keys: vec![(1, 4)],
                selected_key: Some((1, 4)),
                opened_key: Some((1, 4)),
                scroll_offset_y: 123.0,
            },
        });
        app.settings.auto_fullscreen_zip_pdf = true;
        activate(&mut app, &ctx, "A/", true);
        assert!(
            matches!(&app.bookmark_view_state, Some(BookmarkViewState::Opening { target: BookmarkViewReturnTarget::Book(path), grid }) if path == &book && grid.scroll_offset_y == 123.0),
            "activation discarded outer bookmark owner; terminal={terminal}"
        );
        if terminal {
            let main = app.viewer_context_main();
            app.transfer_terminal_close_return(main, ViewerCloseCause::User);
            assert!(matches!(
                app.pending_return_to_parent,
                FullscreenCloseContinuation::Terminal(_)
            ));
            app.close_fullscreen_now();
        } else {
            app.handle_fullscreen_close_request();
        }
        consume_close(&mut app);
        settle(&mut app, &ctx);
        assert_returned(&app, &[], "A/");
        assert!(
            matches!(&app.bookmark_view_state, Some(BookmarkViewState::Opening { target: BookmarkViewReturnTarget::Book(path), grid }) if path == &book && grid.scroll_offset_y == 123.0),
            "close discarded outer bookmark owner; terminal={terminal}, state={:?}",
            app.bookmark_view_state
        );
        assert!(matches!(
            app.handle_grid_parent_folder_action(),
            Some(AddressBarNav::Bookmarks)
        ));
    }
}

#[test]
#[cfg(windows)]
fn section1676_invalidated_frozen_live_terminal_uses_current_list_teardown() {
    use crate::bookmark_browser::BookmarkViewReturnTarget;
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    app.bookmark_view_state = Some(BookmarkViewState::Opening {
        target: BookmarkViewReturnTarget::Book(book.clone()),
        grid: BookmarkViewReturnGridState {
            row_keys: vec![(1, 4)],
            selected_key: Some((1, 4)),
            opened_key: Some((1, 4)),
            scroll_offset_y: 123.0,
        },
    });
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "A/", true);
    assert!(matches!(
        app.bookmark_view_state,
        Some(BookmarkViewState::Opening { .. })
    ));
    app.handle_fullscreen_close_request();
    assert!(matches!(
        app.pending_return_to_parent,
        FullscreenCloseContinuation::LiveZipListReturn(_)
    ));
    app.zip_nav.as_mut().unwrap().clear_list_choices();
    let main = app.viewer_context_main();
    app.transfer_terminal_close_return(main, ViewerCloseCause::User);
    assert!(matches!(
        app.pending_return_to_parent,
        FullscreenCloseContinuation::Terminal(_)
    ));
    let rows = app.items.clone();
    let generation = app.items_generation;
    // Expiration happened before terminal capture. Its teardown therefore has
    // ordinary CurrentList semantics even though the old recipe still travels
    // as Terminal until the consumer discards its stale destination.
    app.close_fullscreen_now();
    assert!(app.fullscreen_idx.is_none());
    assert!(
        app.bookmark_view_state.is_none(),
        "expired chosen destination must not preserve the outer bookmark owner during terminal teardown"
    );
    assert!(matches!(
        app.pending_return_to_parent,
        FullscreenCloseContinuation::Terminal(_)
    ));
    assert_eq!(app.items, rows);
    assert_eq!(app.items_generation, generation);
    let nav = app
        .take_pending_return_to_parent_nav()
        .expect("terminal recipe remains deliverable after teardown");
    assert!(matches!(
        &nav,
        AddressBarNav::ZipListReturn(ZipListReturnRequest::Terminal { .. })
    ));
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    settle(&mut app, &ctx);
    assert!(app.fullscreen_idx.is_none());
    assert!(app.bookmark_view_state.is_none());
    assert!(!app.pending_return_to_parent.is_pending());
    assert_eq!(app.items, rows);
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["A"]);
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_none()
    );
}

#[test]
#[cfg(windows)]
fn section1675_multiwindow_branch_stays_main_list() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/first/001.png", "A/second/001.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    app.settings.detached_viewer_open_images_in_window = true;
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "A/", false);
    assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["A"]);
    assert!(app.active_viewer_context_id().is_none());
    assert!(
        app.items
            .iter()
            .all(|item| matches!(item, GridItem::ZipDir { .. }))
    );
}

#[test]
fn section1675_converted_zipdir_uses_logical_source() {
    use crate::app::book_materialization::{BookScope, ProjectionRecipe};
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let source = app.tmp.path().join("logical.7z");
    let backing = app.tmp.path().join("cache.zip");
    let (service, _owner) = crate::book_composition::BookCompositionService::spawn_at(
        app.tmp.path().join("composition"),
        None,
    )
    .unwrap();
    app.install_book_composition_runtime(service);
    std::fs::write(&source, b"cache fixture; no converter runs").unwrap();
    write_pages(&backing, &["A/001.png", "A/002.png", "B/001.png"]);
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
            3,
            false,
        )
        .unwrap();
    open_list(&mut app, &ctx, &source);
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "A/", true);
    assert_eq!(
        app.archive_source_override.as_deref(),
        Some(source.as_path())
    );
    assert!(
        matches!(&app.projection_recipe, ProjectionRecipe::Book(BookScope::Archive { logical_root, prefix }) if logical_root == &source && prefix == &["A".to_owned()])
    );
    assert_eq!(
        app.book_composition.scope(),
        Some(&crate::book_scope::BookScopeKey::archive(&source, &["A".into()]).unwrap())
    );
    app.handle_fullscreen_close_request_immediate();
    settle(&mut app, &ctx);
    assert_returned(&app, &[], "A/");
    assert_eq!(
        app.archive_source_override.as_deref(),
        Some(source.as_path())
    );
}

#[test]
fn section1676_everything_return_precedence_and_back_out() {
    use crate::global_search_ui::{
        EverythingPosition, SearchAccepted, adopt_everything_for_test,
        assert_everything_return_only_for_test, everything_dataset_for_test,
    };
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/001.png", "B/001.png"]);
    app.indexer_init = IndexerInit::Unavailable;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.auto_fullscreen_zip_pdf = true;
    app.current_folder = Some(app.tmp.path().to_path_buf());
    let results = everything_dataset_for_test(vec![GridItem::ZipFile(book.clone())]);
    adopt_everything_for_test(&mut app, &ctx, "archive inner return", Arc::clone(&results));
    let nav = app.grid_physical_navigation(0, book.clone(), false);
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    settle(&mut app, &ctx);
    assert!(app.everything_has_physical_child());
    activate(&mut app, &ctx, "A/", true);
    assert!(
        app.fullscreen_idx.is_some(),
        "Search physical-child adoption must retain the archive hierarchy"
    );
    app.handle_fullscreen_close_request_immediate();
    settle(&mut app, &ctx);
    assert_returned(&app, &[], "A/");
    assert!(
        matches!(&app.global_search.accepted, SearchAccepted::Everything { position: EverythingPosition::PhysicalChild { logical_source, .. }, .. } if logical_source == &book)
    );
    assert!(app.handle_grid_parent_folder_action().is_none());
    assert_everything_return_only_for_test(&app);
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.everything_has_physical_child() {
        app.poll_search_page_edit_prepare(&ctx);
        assert!(
            Instant::now() < deadline,
            "Everything results return did not adopt"
        );
        std::thread::yield_now();
    }
    settle(&mut app, &ctx);
    assert!(matches!(
        &app.global_search.accepted,
        SearchAccepted::Everything {
            position: EverythingPosition::Root,
            ..
        }
    ));
    assert!(matches!(app.selected.and_then(|idx| app.items.get(idx)),
        Some(GridItem::ZipFile(path)) if path == &book));
    assert!(app.grid_scroll.reveal_requested());
    assert!(
        matches!(&app.global_search.accepted, SearchAccepted::Everything { dataset, .. } if Arc::ptr_eq(dataset, &results))
    );
}

#[test]
#[cfg(windows)]
fn section1676_linked_f12_returns_main_level_and_preserves_same_archive_sibling() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    app.settings.auto_fullscreen_zip_pdf = true;
    app.settings.detached_viewer_enabled = true;
    activate(&mut app, &ctx, "A/", true);
    let items = app.items.clone();
    let nav = app.zip_nav.clone();
    let backing = book.clone();
    let sibling = app.push_window_context_for_test(&ctx, 96_701, move |viewer| {
        viewer.items = items;
        viewer.current_folder = Some(backing);
        viewer.zip_nav = nav;
        viewer.visible_indices = vec![0, 1];
        viewer.fullscreen_idx = Some(1);
        viewer.selected = Some(1);
        viewer.grid_scroll.set_position(321.0);
    });
    let snapshot = |viewer: &mut App| {
        (
            viewer.items.clone(),
            viewer.visible_indices.clone(),
            viewer.items_generation,
            viewer.fullscreen_idx,
            viewer.selected,
            viewer.grid_scroll.offset(),
            viewer.zip_nav.as_ref().unwrap().current().to_vec(),
        )
    };
    let before = app.with_viewer_context_state(sibling, snapshot).unwrap();
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.fs_viewport_presentation = Some(ViewerPresentation::DetachedWindow);
    app.fs_viewport_shown = true;
    app.handle_fullscreen_close_request_immediate();
    settle(&mut app, &ctx);
    assert_returned(&app, &[], "A/");
    let generation = app.items_generation;
    assert!(app.take_pending_return_to_parent_nav().is_none());
    assert_eq!(
        app.items_generation, generation,
        "close return is consumed once"
    );
    assert_eq!(
        app.with_viewer_context_state(sibling, snapshot).unwrap(),
        before
    );
}

#[test]
fn section1675_snapshot_zipdir_keeps_explicit_list_navigation() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let shelf = app.tmp.path().join("shelf");
    std::fs::create_dir(&shelf).unwrap();
    let book = shelf.join("outer.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
    app.indexer_init = IndexerInit::Unavailable;
    app.load_folder(shelf);
    settle(&mut app, &ctx);
    app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    settle(&mut app, &ctx);
    assert!(app.is_snapshot_active());
    open_list(&mut app, &ctx, &book);
    assert!(
        app.is_snapshot_active(),
        "the captured archive remains inside the fixed scope"
    );
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "A/", false);
    assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["A"]);
    assert!(app.fullscreen_idx.is_none());
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_none()
    );
}

#[test]
fn section1675_unsafe_structural_prefix_keeps_existing_list_navigation() {
    for auto in [false, true] {
        for multi_window in [false, true] {
            for branch in [false, true] {
                let mut app = setup_app_for_test();
                let ctx = egui::Context::default();
                let book = app.tmp.path().join("outer.zip");
                write_pages(
                    &book,
                    if branch {
                        &["part:one/A/001.png", "part:one/B/001.png", "other/001.png"]
                    } else {
                        &["part:one/001.png", "part:one/002.png", "other/001.png"]
                    },
                );
                open_list(&mut app, &ctx, &book);
                app.settings.auto_fullscreen_zip_pdf = auto;
                app.settings.detached_viewer_open_images_in_window = multi_window;
                let feedback_start = app.feedback_toasts_for_test.len();
                activate(&mut app, &ctx, "part:one/", false);
                assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["part:one"]);
                assert!(app.fullscreen_idx.is_none());
                assert_eq!(app.items.len(), 2);
                assert!(app.items.iter().all(|item| if branch {
                    matches!(item, GridItem::ZipDir { .. })
                } else {
                    matches!(item, GridItem::ZipImage { .. })
                }));
                assert!(
                    app.zip_nav
                        .as_ref()
                        .unwrap()
                        .chosen_list_return_plan()
                        .is_some()
                );
                let feedback = &app.feedback_toasts_for_test[feedback_start..];
                let book_candidate = auto && !multi_window && !branch;
                assert_eq!(
                    feedback
                        .iter()
                        .filter(|(text, kind)| text
                            == "この内部フォルダは本として開けないため、一覧を表示します"
                            && matches!(kind, crate::feedback::FeedbackKind::Info))
                        .count(),
                    usize::from(book_candidate)
                );
                if book_candidate {
                    // The normal composition poll must reuse its Unsupported
                    // outcome, not publish a second warning for this generation.
                    assert!(
                        !feedback.iter().any(|(text, _)| text
                            == crate::book_composition::UNSUPPORTED_COMPOSITION_NOTICE)
                    );
                }
            }
        }
    }
}

#[test]
fn section1676_sort_and_pin_rematerialization_keep_chosen_list() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    app.settings.auto_fullscreen_zip_pdf = false;
    activate(&mut app, &ctx, "A/", false);
    let plan = app
        .zip_nav
        .as_ref()
        .unwrap()
        .chosen_list_return_plan()
        .unwrap();
    app.settings.sort_order = crate::settings::SortOrder::FileNameDesc;
    app.apply_sort_change_reload();
    settle(&mut app, &ctx);
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .matches_list_return_plan(&plan)
    );
    // This is the existing pin-change refresh path: it replaces the same
    // level's rows without enumerating or rebuilding its tree/stack.
    app.refresh_current_zip_level_preserving_position();
    settle(&mut app, &ctx);
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .matches_list_return_plan(&plan)
    );
    app.settings.auto_fullscreen_zip_pdf = true;
    app.open_fullscreen(0, HistoryTrigger::UserChosen);
    app.handle_fullscreen_close_request_immediate();
    settle(&mut app, &ctx);
    assert_returned(&app, &[], "A/");
}

#[test]
fn section1676_real_refresh_stale_live_plan_closes_to_current_list() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    app.settings.auto_fullscreen_zip_pdf = false;
    activate(&mut app, &ctx, "A/", false);
    let plan = app
        .zip_nav
        .as_ref()
        .unwrap()
        .chosen_list_return_plan()
        .unwrap();
    app.reload_top_level_grid(&ctx);
    settle(&mut app, &ctx);
    assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["A"]);
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_none()
    );
    assert!(
        !app.zip_nav
            .as_ref()
            .unwrap()
            .matches_list_return_plan(&plan)
    );
    app.open_fullscreen(1, HistoryTrigger::UserChosen);
    app.spread_popup_open = true;
    let before = (app.items.clone(), app.items_generation);
    app.pending_return_to_parent = FullscreenCloseContinuation::LiveZipListReturn(plan);
    let nav = app
        .take_pending_return_to_parent_nav()
        .expect("frozen Live request issues its delivery authority");
    assert!(matches!(
        &nav,
        AddressBarNav::ZipListReturn(ZipListReturnRequest::Live { .. })
    ));
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    assert_eq!((app.items.clone(), app.items_generation), before);
    assert!(
        app.fullscreen_idx.is_none(),
        "an expired Live destination still fulfills its accepted Close at CurrentList"
    );
    assert!(
        !app.spread_popup_open,
        "stale Live close still runs fullscreen teardown"
    );
    assert!(!app.pending_return_to_parent.is_pending());
    assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["A"]);
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_none()
    );
}

#[test]
fn section1676_physical_history_roundtrip_does_not_restore_list_choice() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    let other = app.tmp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("other.png"), png()).unwrap();
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
    app.active_quick_folder_slot = None;
    open_list(&mut app, &ctx, &book);
    app.settings.auto_fullscreen_zip_pdf = false;
    activate(&mut app, &ctx, "A/", false);
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_some()
    );
    app.load_folder(other.clone());
    settle(&mut app, &ctx);
    assert!(app.zip_nav.is_none());
    app.dispatch_folder_history_input_for_test(FolderHistoryDirection::Back);
    settle(&mut app, &ctx);
    assert_eq!(app.current_folder.as_deref(), Some(book.as_path()));
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_none()
    );
    if app.zip_nav.as_ref().unwrap().at_root() {
        app.zip_nav_enter("A/");
        settle(&mut app, &ctx);
    }
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_none()
    );
    app.dispatch_folder_history_input_for_test(FolderHistoryDirection::Forward);
    settle(&mut app, &ctx);
    assert_eq!(app.current_folder.as_deref(), Some(other.as_path()));
    assert!(app.zip_nav.is_none());
}

#[test]
fn section1676_same_archive_bookmark_jump_clears_even_already_installed_choice() {
    use crate::book_bookmarks::{BookBookmark, BookContainerKind, PageIdentity};
    for (entry, expected_prefix) in [("A/002.png", "A"), ("B/001.png", "B")] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("outer.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
        open_list(&mut app, &ctx, &book);
        app.settings.auto_fullscreen_zip_pdf = true;
        activate(&mut app, &ctx, "A/", true);
        let bookmark = BookBookmark {
            id: 1,
            container_key: crate::book_bookmarks::container_key(&book),
            container_path: book.clone(),
            container_kind: BookContainerKind::Zip,
            page_identity: PageIdentity::ArchiveEntry(entry.into()),
            page_index_hint: usize::from(expected_prefix == "A"),
            created_at_ms: 1,
            title: None,
        };
        app.jump_to_current_book_bookmark(&ctx, &bookmark);
        settle(&mut app, &ctx);
        assert_eq!(app.zip_nav.as_ref().unwrap().current(), &[expected_prefix]);
        assert!(
            matches!(&app.items[app.fullscreen_idx.unwrap()], GridItem::ZipImage { entry_name, .. } if entry_name == entry)
        );
        assert!(
            app.zip_nav
                .as_ref()
                .unwrap()
                .chosen_list_return_plan()
                .is_none()
        );
        app.handle_fullscreen_close_request();
        assert!(
            matches!(app.take_pending_return_to_parent_nav(), Some(AddressBarNav::Direct(path, _)) if Some(path.as_path()) == book.parent())
        );
    }
}

#[test]
fn section1676_required_same_archive_open_does_not_retain_list_choice() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/001.png", "B/001.png", "B/002.png"]);
    open_list(&mut app, &ctx, &book);
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "A/", true);
    assert!(app.start_physical_history_transition(
        PhysicalHistoryIntent::RequiredFullscreen {
            target: crate::snapshot::SnapshotTarget::ZipImage {
                zip_path: book.clone(),
                entry_name: "B/002.png".into(),
            },
            history_trigger: HistoryTrigger::UserChosen,
            navigation_purpose: FsNavigationPurpose::Ordinary,
        },
        book,
        StartupListIntent::PageContinuation,
    ));
    settle(&mut app, &ctx);
    assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["B"]);
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_none()
    );
    assert!(
        matches!(&app.items[app.fullscreen_idx.unwrap()], GridItem::ZipImage { entry_name, .. } if entry_name == "B/002.png")
    );
}

#[test]
fn section1675_file_type_policy_classifies_only_visible_direct_pages() {
    for all_excluded in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("outer.zip");
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(8, 12)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        std::fs::write(
            &book,
            zip_bytes(&[
                ("A/001.png", png()),
                ("A/002.jpg", jpeg.into_inner()),
                ("B/001.png", png()),
            ]),
        )
        .unwrap();
        app.settings
            .file_type_visibility
            .excluded_extensions
            .insert("png".into());
        if all_excluded {
            app.settings
                .file_type_visibility
                .excluded_extensions
                .insert("jpg".into());
        }
        open_list(&mut app, &ctx, &book);
        app.settings.auto_fullscreen_zip_pdf = true;
        activate(&mut app, &ctx, "A/", !all_excluded);
        assert_eq!(app.fullscreen_idx.is_some(), !all_excluded);
        if !all_excluded {
            assert_eq!(app.items[app.fullscreen_idx.unwrap()].name(), "002.jpg");
        }
        assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["A"]);
    }
}

#[cfg(windows)]
fn settle_collection_root(
    app: &mut App,
    ctx: &egui::Context,
    id: crate::collection_store::CollectionId,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_collection_ui(ctx);
        app.poll_collection_grid(ctx);
        app.poll_collection_history_transition(ctx);
        app.poll_sidecar_restore(ctx);
        let ready = app
            .top_level_grid_view
            .collection_session()
            .is_some_and(|session| {
                session.identity.collection_id == id
                    && matches!(
                        session.position,
                        top_level_grid_view::CollectionGridPosition::Root
                    )
                    && matches!(
                        session.load,
                        top_level_grid_view::CollectionGridLoadState::Ready(_)
                    )
                    && session.installed_items_generation == Some(app.items_generation)
            });
        if ready && !app.sidecar_restore_active() {
            break;
        }
        assert!(Instant::now() < deadline, "collection root did not settle");
        std::thread::yield_now();
    }
}

#[test]
#[cfg(windows)]
fn section1676_collection_live_and_terminal_close_preserve_owner_and_back_out() {
    use crate::app::top_level_grid_view::CollectionGridPosition;
    use crate::collection_store::{
        CollectionRegistration, CollectionResolvedKind, CollectionStoreRuntime,
    };
    for terminal in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        app.indexer_init = IndexerInit::Unavailable;
        app.active_quick_folder_slot = None;
        app.settings.detached_viewer_open_images_in_window = false;
        app.settings.auto_fullscreen_zip_pdf = false;
        let shelf = app.tmp.path().join("shelf");
        std::fs::create_dir(&shelf).unwrap();
        let book = shelf.join("outer.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
        let runtime =
            CollectionStoreRuntime::start_at(app.tmp.path().join("collection.db")).unwrap();
        let client = runtime.client();
        app.install_collection_runtime(runtime);
        let deadline = Instant::now() + Duration::from_secs(30);
        while !matches!(app.collection_store_client_for_read(), Ok(Some(_))) {
            app.poll_collection_ui(&ctx);
            assert!(
                Instant::now() < deadline,
                "collection runtime did not initialize"
            );
            std::thread::yield_now();
        }
        let created = client
            .create_collection("Internal book return".into())
            .unwrap()
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .unwrap();
        let snapshot = client
            .add_batch(
                created.collection_id(),
                created.revision(),
                vec![
                    CollectionRegistration::from_trusted_path(
                        &shelf,
                        CollectionResolvedKind::Folder,
                    )
                    .unwrap(),
                ],
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .unwrap()
            .snapshot;
        let id = snapshot.collection_id();
        app.open_collection_grid_from_navigation(id);
        settle_collection_root(&mut app, &ctx, id);
        let shelf_owner = app.collection_grid_physical_load_owner(0, &shelf).unwrap();
        assert!(app.load_folder_with_scan_owned(
            shelf.clone(),
            None,
            OpenRequestOwner::CollectionGridPhysical(shelf_owner),
            StartupListIntent::ExplicitList,
        ));
        settle(&mut app, &ctx);
        let book_idx = app
            .items
            .iter()
            .position(|item| item.drag_source_path() == Some(book.as_path()))
            .unwrap();
        let book_owner = app
            .collection_grid_physical_load_owner(book_idx, &book)
            .unwrap();
        assert!(matches!(
            app.load_folder_or_convert_archive_with_auto_fullscreen_owned(
                book.clone(),
                false,
                OpenRequestOwner::CollectionGridPhysical(book_owner),
                StartupListIntent::ExplicitList,
            ),
            FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
        ));
        settle(&mut app, &ctx);
        let position_before = app
            .top_level_grid_view
            .collection_session()
            .unwrap()
            .position
            .clone();
        assert!(
            matches!(&position_before, CollectionGridPosition::PhysicalSource { path, .. } if path == &shelf)
        );
        app.settings.auto_fullscreen_zip_pdf = true;
        activate(&mut app, &ctx, "A/", true);
        if terminal {
            let main = app.viewer_context_main();
            app.transfer_terminal_close_return(main, ViewerCloseCause::User);
            assert!(matches!(
                app.pending_return_to_parent,
                FullscreenCloseContinuation::Terminal(_)
            ));
            app.close_fullscreen_now();
        } else {
            app.handle_fullscreen_close_request();
        }
        consume_close(&mut app);
        settle(&mut app, &ctx);
        assert_returned(&app, &[], "A/");
        let session = app.top_level_grid_view.collection_session().unwrap();
        assert_eq!(session.identity.collection_id, id);
        assert_eq!(session.position, position_before);
        let nav = app
            .handle_grid_parent_folder_action()
            .expect("ZIP root must retain collection physical-parent return");
        assert!(matches!(&nav, AddressBarNav::CollectionSource { path, .. } if path == &shelf));
        assert!(app.apply_fullscreen_close_nav_immediate(nav));
        settle(&mut app, &ctx);
        assert_eq!(app.current_folder.as_deref(), Some(shelf.as_path()));
        assert!(app.zip_nav.is_none());
        assert!(
            matches!(&app.items[app.selected.unwrap()], GridItem::ZipFile(path) if path == &book)
        );
        let nav = app
            .handle_grid_parent_folder_action()
            .expect("registered folder must retain collection-root return");
        assert!(
            matches!(&nav, AddressBarNav::Collection(restore) if restore.identity.collection_id == id)
        );
        assert!(app.apply_fullscreen_close_nav_immediate(nav));
        settle_collection_root(&mut app, &ctx, id);
        assert!(
            matches!(&app.items[app.selected.unwrap()], GridItem::Folder(path) if path == &shelf)
        );
    }
}

#[cfg(windows)]
fn close_chosen_root(app: &mut App, ctx: &egui::Context, terminal: bool) {
    if terminal {
        let main = app.viewer_context_main();
        app.transfer_terminal_close_return(main, ViewerCloseCause::User);
        assert!(matches!(
            app.pending_return_to_parent,
            FullscreenCloseContinuation::Terminal(_)
        ));
        app.close_fullscreen_now();
    } else {
        app.handle_fullscreen_close_request();
    }
    consume_close(app);
    settle(app, ctx);
    assert_returned(app, &[], "A/");
}

#[test]
#[cfg(windows)]
fn section1676_reading_history_owner_survives_chosen_live_and_terminal_return() {
    for terminal in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("outer.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
        open_list(&mut app, &ctx, &book);
        // The same reservation fixture used by section1328_session's deep ZIP
        // coverage; it has no live history browser rows to overwrite.
        app.reading_history_return_from = Some(reading_history_return_for_test(book.clone()));
        app.settings.auto_fullscreen_zip_pdf = true;
        activate(&mut app, &ctx, "A/", true);
        assert!(
            app.reading_history_back_nav().is_none(),
            "deep ZIP is not the history root"
        );
        close_chosen_root(&mut app, &ctx, terminal);
        assert_eq!(
            app.reading_history_return_from
                .as_ref()
                .map(|owner| &owner.container),
            Some(&book)
        );
        assert!(matches!(
            app.handle_grid_parent_folder_action(),
            Some(AddressBarNav::ReadingHistory(
                ReadingHistoryEntrance::Return(_)
            ))
        ));
        assert!(
            app.reading_history_return_from.is_some(),
            "resolving Backspace must not lose its owned entrance"
        );
    }
}

#[cfg(windows)]
fn settle_rating(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_rating_view();
        app.poll_collection_history_transition(ctx);
        app.poll_sidecar_restore(ctx);
        if app.rating_view_pending.is_none()
            && !app.sidecar_restore_active()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
        {
            break;
        }
        assert!(Instant::now() < deadline, "rating owner did not settle");
        std::thread::yield_now();
    }
}

#[test]
#[cfg(windows)]
fn section1676_rating_owner_survives_chosen_live_and_terminal_return() {
    for terminal in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("outer.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
        app.indexer_init = IndexerInit::Unavailable;
        app.active_quick_folder_slot = None;
        app.settings.auto_fullscreen_zip_pdf = false;
        app.settings.detached_viewer_open_images_in_window = false;
        let meta = crate::rating_db::RatingMeta::new(crate::rating_db::RatingItemKind::ZipFile)
            .with_source_path(&book);
        app.rating_db
            .as_ref()
            .unwrap()
            .set_user_rating(&crate::adjustment_db::normalize_path(&book), 3, Some(&meta))
            .unwrap();
        app.current_folder = Some(app.tmp.path().to_path_buf());
        app.enter_rating_view(3);
        settle_rating(&mut app, &ctx);
        assert!(app.items_are_rating_view);
        let owner = app.rating_view_physical_load_owner(&book).unwrap();
        assert!(app.start_rating_physical_open(owner, StartupListIntent::ExplicitList));
        settle(&mut app, &ctx);
        let before = app.rating_view_nav_stack.clone();
        assert!(!before.is_empty());
        app.settings.auto_fullscreen_zip_pdf = true;
        activate(&mut app, &ctx, "A/", true);
        close_chosen_root(&mut app, &ctx, terminal);
        assert_eq!(app.rating_view_nav_stack, before);
        assert!(matches!(
            app.grid_parent_action(),
            crate::app::grid_parent::GridParentAction::RatingBack
        ));
        assert!(
            app.handle_grid_parent_folder_action().is_none(),
            "rating Backspace dispatches its existing owner directly"
        );
        settle_rating(&mut app, &ctx);
        assert!(app.items_are_rating_view);
        assert!(app.rating_view_nav_stack.is_empty());
        assert!(
            matches!(&app.items[app.selected.unwrap()], GridItem::ZipFile(path) if path == &book)
        );
    }
}

#[cfg(windows)]
fn settle_smart(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_smart_folder(ctx);
        app.poll_collection_history_transition(ctx);
        app.poll_zip_enumerate();
        app.poll_sidecar_restore(ctx);
        if !app.smart_folder_busy()
            && !app.sidecar_restore_active()
            && app.zip_enumerate_pending.is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
        {
            break;
        }
        assert!(Instant::now() < deadline, "smart owner did not settle");
        std::thread::yield_now();
    }
}

#[test]
#[cfg(windows)]
fn section1676_smart_owner_survives_chosen_live_and_terminal_return() {
    for terminal in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let shelf = app.tmp.path().join("shelf");
        std::fs::create_dir(&shelf).unwrap();
        let book = shelf.join("outer.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
        app.indexer_init = IndexerInit::Unavailable;
        app.active_quick_folder_slot = None;
        app.settings.auto_fullscreen_zip_pdf = false;
        app.settings.detached_viewer_open_images_in_window = false;
        let mut definition = crate::settings::SmartFolderDefinition::new("Chosen archive return");
        definition.rules.push(crate::settings::SmartFolderRule::new(
            shelf,
            true,
            Default::default(),
        ));
        let id = definition.id;
        app.settings.smart_folders = vec![definition];
        app.open_smart_folder(id, false);
        settle_smart(&mut app, &ctx);
        let kind = app.smart_physical_target_kind(&book).unwrap();
        assert!(
            app.begin_smart_physical_navigation(
                book.clone(),
                kind,
                false,
                None,
                None,
                StartupListIntent::ExplicitList,
            )
            .is_ok()
        );
        settle_smart(&mut app, &ctx);
        let before = app.top_level_grid_view.smart_folder().unwrap().clone();
        app.settings.auto_fullscreen_zip_pdf = true;
        activate(&mut app, &ctx, "A/", true);
        close_chosen_root(&mut app, &ctx, terminal);
        assert_eq!(app.top_level_grid_view.smart_folder(), Some(&before));
        let nav = app
            .handle_grid_parent_folder_action()
            .expect("smart physical book retains its root return");
        assert!(
            matches!(&nav, AddressBarNav::Direct(path, _) if path == &crate::app::smart_folder::smart_folder_synthetic_path(id))
        );
        assert!(app.apply_fullscreen_close_nav_immediate(nav));
        settle_smart(&mut app, &ctx);
        assert!(matches!(
            app.top_level_grid_view.smart_folder().unwrap().position,
            top_level_grid_view::SmartFolderPosition::Root
        ));
        assert!(
            app.items
                .iter()
                .any(|item| matches!(item, GridItem::ZipFile(path) if path == &book))
        );
    }
}

#[test]
#[cfg(windows)]
fn section1676_subfolder_origin_keeps_existing_physical_backout_after_chosen_close() {
    for terminal in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let shelf = app.tmp.path().join("shelf");
        std::fs::create_dir(&shelf).unwrap();
        let book = shelf.join("outer.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
        app.indexer_init = IndexerInit::Unavailable;
        app.active_quick_folder_slot = None;
        app.settings.auto_fullscreen_zip_pdf = false;
        app.settings.detached_viewer_open_images_in_window = false;
        app.load_folder(shelf.clone());
        settle(&mut app, &ctx);
        app.start_subfolder_expansion_scan_roots(shelf.clone(), vec![shelf.clone()]);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            app.poll_subfolder_expansion(&ctx);
            app.poll_sidecar_restore(&ctx);
            if !app.subfolder_expansion_busy() && !app.sidecar_restore_active() {
                break;
            }
            assert!(Instant::now() < deadline, "subfolder scan did not settle");
            std::thread::yield_now();
        }
        assert!(app.items_are_subfolder_expansion_view);
        let idx = app
            .items
            .iter()
            .position(|item| matches!(item, GridItem::ZipFile(path) if path == &book))
            .unwrap();
        let nav = app.grid_physical_navigation(idx, book.clone(), false);
        assert!(app.apply_fullscreen_close_nav_immediate(nav));
        settle(&mut app, &ctx);
        assert!(!app.items_are_subfolder_expansion_view);
        assert!(
            app.folder_nav_subfolder_restore.is_some(),
            "physical adoption parks the expansion in its existing history owner"
        );
        app.settings.auto_fullscreen_zip_pdf = true;
        activate(&mut app, &ctx, "A/", true);
        close_chosen_root(&mut app, &ctx, terminal);
        assert!(!app.items_are_subfolder_expansion_view);
        assert!(app.folder_nav_subfolder_restore.is_some());
        let nav = app
            .handle_grid_parent_folder_action()
            .expect("existing physical parent after ZIP root");
        assert!(matches!(&nav, AddressBarNav::Direct(path, _) if path == &shelf));
        assert!(app.apply_fullscreen_close_nav_immediate(nav));
        settle(&mut app, &ctx);
        assert_eq!(app.current_folder.as_deref(), Some(shelf.as_path()));
        assert!(app.folder_nav_subfolder_restore.is_some());
        assert!(!app.items_are_subfolder_expansion_view);
    }
}

#[cfg(windows)]
fn headless_update_frame(
    app: &mut App,
    backend: &mut eframe::HeadlessEpiIntegration,
    frame: u64,
    events: Vec<egui::Event>,
) {
    headless_update_frame_with_focus(app, backend, frame, events, false);
}

#[cfg(windows)]
fn headless_update_frame_with_focus(
    app: &mut App,
    backend: &mut eframe::HeadlessEpiIntegration,
    frame: u64,
    events: Vec<egui::Event>,
    focused: bool,
) {
    let mut raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 800.0),
        )),
        time: Some(frame as f64 / 60.0),
        focused,
        events,
        ..Default::default()
    };
    raw.viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .focused = Some(focused);
    backend.update(app, raw).drop_without_applying_deltas();
}

#[test]
#[cfg(windows)]
fn section1676_in_window_update_early_close_delivers_chosen_return_once() {
    let _native_input = crate::key_input::lock_test_input();
    crate::key_input::clear_test_frame();
    let _focus = crate::test_native_focus::scoped(1, 1, true);
    let mut app = setup_app_for_test();
    app.initialized = true;
    app.settings.update_check_enabled = false;
    app.settings.minimize_to_tray_on_close = false;
    app.last_main_focused = false;
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    app.settings.video_in_window_mode = true;
    app.settings.detached_viewer_enabled = false;
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "A/", true);
    assert_eq!(app.viewer_presentation, ViewerPresentation::MainWindow);
    assert!(app.fullscreen_embedded_still_active());
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
    // Warm a focused embedded frame, then deliver Escape with no pending
    // navigation. Root input delegates to the embedded fullscreen renderer;
    // only that rendering pass can produce the Close for the late consumer.
    headless_update_frame_with_focus(&mut app, &mut backend, 1, Vec::new(), true);
    assert!(app.fullscreen_embedded_still_active());
    assert!(!app.pending_return_to_parent.is_pending());
    headless_update_frame_with_focus(
        &mut app,
        &mut backend,
        2,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        true,
    );
    settle(&mut app, &ctx);
    assert_returned(&app, &[], "A/");
    assert!(!app.fullscreen_embedded_still_active());
    let generation = app.items_generation;
    headless_update_frame_with_focus(
        &mut app,
        &mut backend,
        3,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        true,
    );
    assert_returned(&app, &[], "A/");
    assert_eq!(
        app.items_generation, generation,
        "early close must consume its return only once"
    );
}

#[test]
#[cfg(windows)]
fn section1676_embedded_page_turn_barrier_keeps_pending_close_for_update_delivery() {
    let _native_input = crate::key_input::lock_test_input();
    crate::key_input::clear_test_frame();
    let mut app = setup_app_for_test();
    app.initialized = true;
    app.settings.update_check_enabled = false;
    app.settings.minimize_to_tray_on_close = false;
    app.last_main_focused = false;
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
    let book = app.tmp.path().join("outer.zip");
    write_pages(
        &book,
        &[
            "A/001.png",
            "A/002.png",
            "A/003.png",
            "A/004.png",
            "B/001.png",
        ],
    );
    open_list(&mut app, &ctx, &book);
    app.settings.video_in_window_mode = true;
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "A/", true);
    app.spread_mode = crate::settings::SpreadMode::Single;
    app.reading_flow = crate::settings::ReadingFlow::Paged;
    // Replace decoding only. The archive level, chosen return, registry and
    // close request are production-owned; real painted receipts create the turn.
    for idx in 0..4 {
        app.cancel_fs_page_load(idx);
        crate::app::source_texture_test_support::seed(&mut app, &ctx, idx);
        app.record_page_dims_for_spread(idx, (400, 600));
    }
    app.handle_fullscreen_close_request();
    assert!(matches!(
        app.pending_return_to_parent,
        FullscreenCloseContinuation::LiveZipListReturn(_)
    ));
    // True-close cancels old page-turn state, so establish this specific ingress
    // interleaving after request acceptance using the existing painted fixture.
    crate::ui_fullscreen::page_turn_submission_tests::seed_leaf(&ctx, &mut app);
    assert!(app.page_turn_barrier_active());
    assert!(app.fullscreen_embedded_still_active());
    assert!(matches!(
        app.pending_return_to_parent,
        FullscreenCloseContinuation::LiveZipListReturn(_)
    ));
    ctx.run_pass(Default::default(), |ctx| {
        let (handled, forwarded) = app.fullscreen_root_input_for_frame(ctx);
        assert!(
            handled,
            "embedded barrier must suppress the grid keyboard consumer"
        );
        assert!(
            forwarded.is_none(),
            "embedded presentation owns input in its main Context"
        );
        assert!(app.pending_return_to_parent.is_pending());
    })
    .drop_without_applying_deltas();
    assert!(app.page_turn_barrier_active());
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
    headless_update_frame(&mut app, &mut backend, 1, Vec::new());
    settle(&mut app, &ctx);
    assert_returned(&app, &[], "A/");
    assert!(!app.page_turn_barrier_active());
    assert!(!app.fullscreen_embedded_still_active());
}

#[test]
#[cfg(windows)]
fn section1676_pending_main_live_return_preserves_newly_active_sibling_session() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png", "B/002.png"]);
    open_list(&mut app, &ctx, &book);
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "A/", true);
    app.handle_fullscreen_close_request();
    assert!(matches!(
        app.pending_return_to_parent,
        FullscreenCloseContinuation::LiveZipListReturn(_)
    ));
    // A real independent descriptor open publishes its context/window binding
    // and active session after main has already accepted its own Live close.
    // The descriptor helper is the independent route's implementation seam;
    // its real caller enables this open policy before initial fullscreen open.
    app.settings.detached_viewer_open_images_in_window = true;
    ctx.set_embed_viewports(false);
    assert!(app.start_active_detached_book_context(
        ViewerContextDescriptor::Zip {
            path: book.clone(),
            entry_name: Some("B/001.png".into()),
            archive_source_override: None,
        },
        &ctx,
        None,
        None,
    ));
    let sibling = app.active_viewer_context_id().unwrap();
    assert_ne!(sibling, app.viewer_context_main());
    app.with_active_viewer_context(|viewer| {
        settle(viewer, &ctx);
        assert!(
            viewer.fullscreen_idx.is_some(),
            "sibling descriptor must open its explicit leaf"
        );
        assert_eq!(
            viewer.viewer_presentation,
            ViewerPresentation::DetachedWindow
        );
        assert!(viewer.detached_viewer_independent_active);
    })
    .unwrap();
    assert_eq!(app.active_viewer_context_id(), Some(sibling));
    ctx.run_ui(Default::default(), |ui| {
        assert!(app.update_active_viewer_context(ui, None).updated);
    })
    .drop_without_applying_deltas();
    let session = app.active_detached_session.expect("real sibling session");
    assert_eq!(session.content_phase, DetachedSessionContentPhase::Ready);
    let window = session.window_id;
    let viewport = App::detached_image_window_viewport_id(window);
    ctx.run_ui(Default::default(), |ui| {
        ctx.show_viewport_deferred(viewport, egui::ViewportBuilder::default(), |_, _| {});
        app.capture_detached_viewport_registration(ui.ctx(), window);
    })
    .drop_without_applying_deltas();
    let registration = format!(
        "{:?}",
        ctx.viewport_registration(viewport)
            .expect("registered sibling viewport")
    );
    let runtime = format!(
        "{:?}",
        app.detached_window_manager
            .runtime(window)
            .expect("sibling runtime")
    );
    let session_before = (session.window_id, session.source, session.content_phase);
    let state = |viewer: &mut App| {
        (
            viewer.items.clone(),
            viewer.visible_indices.clone(),
            viewer.items_generation,
            viewer.selected,
            viewer.fullscreen_idx,
            viewer.grid_scroll.offset(),
            viewer.fs_viewport_shown,
            viewer.fs_viewport_presentation,
            viewer.viewer_presentation,
            viewer.zip_nav.as_ref().unwrap().current().to_vec(),
        )
    };
    let before = app.with_viewer_context_state(sibling, state).unwrap();
    let pending = app
        .with_viewer_context_state(sibling, |viewer| {
            viewer
                .fs_pending
                .iter()
                .map(|(&idx, value)| (idx, Arc::clone(&value.cancel)))
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert!(matches!(
        app.pending_return_to_parent,
        FullscreenCloseContinuation::LiveZipListReturn(_)
    ));
    consume_close(&mut app);
    settle(&mut app, &ctx);
    assert_returned(&app, &[], "A/");
    assert_eq!(app.active_viewer_context_id(), Some(sibling));
    assert_eq!(
        app.active_detached_session
            .map(|s| (s.window_id, s.source, s.content_phase)),
        Some(session_before)
    );
    assert_eq!(
        format!(
            "{:?}",
            app.detached_window_manager
                .runtime(window)
                .expect("sibling runtime survived main close")
        ),
        runtime
    );
    assert_eq!(
        format!(
            "{:?}",
            ctx.viewport_registration(viewport)
                .expect("main close did not retire sibling viewport")
        ),
        registration
    );
    assert_eq!(
        app.locate_window_context(window).map(|(owner, _)| owner),
        Some(sibling)
    );
    assert_eq!(
        app.with_viewer_context_state(sibling, state).unwrap(),
        before
    );
    app.with_viewer_context_state(sibling, |viewer| {
        for (idx, cancel) in &pending {
            let current = viewer
                .fs_pending
                .get(idx)
                .expect("main close did not cancel sibling load");
            assert!(Arc::ptr_eq(cancel, &current.cancel));
            assert!(!cancel.load(Ordering::Relaxed));
        }
    })
    .unwrap();
}

#[test]
#[cfg(windows)]
fn section1676_stale_live_choice_delivers_current_list_through_real_update() {
    let _native_input = crate::key_input::lock_test_input();
    crate::key_input::clear_test_frame();
    let mut app = setup_app_for_test();
    app.initialized = true;
    app.settings.update_check_enabled = false;
    app.settings.minimize_to_tray_on_close = false;
    app.last_main_focused = false;
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    app.settings.video_in_window_mode = true;
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "A/", true);
    app.handle_fullscreen_close_request();
    assert!(matches!(
        app.pending_return_to_parent,
        FullscreenCloseContinuation::LiveZipListReturn(_)
    ));
    // A refresh/restore removes the old tree's Chosen facts. Exercise the
    // invalidation boundary before delivery while the current reader is still
    // present; do not create another open or resolve another return plan.
    app.zip_nav.as_mut().unwrap().clear_list_choices();
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_none()
    );
    let before = (app.items.clone(), app.items_generation, app.input_seq);
    app.spread_popup_open = true;
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
    headless_update_frame(&mut app, &mut backend, 1, Vec::new());
    settle(&mut app, &ctx);
    assert!(app.fullscreen_idx.is_none());
    assert!(!app.spread_popup_open);
    assert!(!app.pending_return_to_parent.is_pending());
    assert_eq!(app.current_folder.as_deref(), Some(book.as_path()));
    assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["A"]);
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_none()
    );
    assert_eq!(
        (app.items.clone(), app.items_generation, app.input_seq),
        before,
        "stale Live delivers Close at the current rows without opening or installing another destination"
    );
    let generation = app.items_generation;
    headless_update_frame(&mut app, &mut backend, 2, Vec::new());
    assert!(app.fullscreen_idx.is_none());
    assert!(!app.pending_return_to_parent.is_pending());
    assert_eq!(app.items_generation, generation);
}

#[test]
#[cfg(windows)]
fn section1676_stale_terminal_return_cannot_close_a_later_reader() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "A/", true);
    let main = app.viewer_context_main();
    app.transfer_terminal_close_return(main, ViewerCloseCause::User);
    assert!(matches!(
        app.pending_return_to_parent,
        FullscreenCloseContinuation::Terminal(_)
    ));
    app.close_fullscreen_now();
    let nav = app
        .take_pending_return_to_parent_nav()
        .expect("terminal return outlives the closed reader");
    assert!(matches!(
        &nav,
        AddressBarNav::ZipListReturn(ZipListReturnRequest::Terminal { .. })
    ));
    app.zip_nav.as_mut().unwrap().clear_list_choices();
    // A later real open has its own fullscreen lifetime. Delivery of the old
    // Terminal recipe must only discard the stale navigation, never close it.
    app.open_fullscreen(1, HistoryTrigger::UserChosen);
    app.spread_popup_open = true;
    let before = (
        app.items.clone(),
        app.items_generation,
        app.fullscreen_idx,
        app.input_seq,
    );
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    assert_eq!(
        (
            app.items.clone(),
            app.items_generation,
            app.fullscreen_idx,
            app.input_seq
        ),
        before
    );
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.spread_popup_open);
    assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["A"]);
    assert!(!app.pending_return_to_parent.is_pending());
}

#[cfg(windows)]
fn archive_return_lifetime_env() -> (AppTestEnvForTest, egui::Context, PathBuf) {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "A/003.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "A/", true);
    app.open_fullscreen(1, HistoryTrigger::UserChosen);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_some()
    );
    (app, ctx, book)
}

#[cfg(windows)]
fn take_terminal_archive_return(app: &mut App) -> AddressBarNav {
    let main = app.viewer_context_main();
    app.transfer_terminal_close_return(main, ViewerCloseCause::User);
    assert!(matches!(
        app.pending_return_to_parent,
        FullscreenCloseContinuation::Terminal(_)
    ));
    app.close_fullscreen_now();
    assert!(app.fullscreen_idx.is_none());
    let nav = app
        .take_pending_return_to_parent_nav()
        .expect("real Terminal issues a navigation authority");
    assert!(matches!(
        &nav,
        AddressBarNav::ZipListReturn(ZipListReturnRequest::Terminal { .. })
    ));
    assert!(matches!(
        app.pending_return_to_parent,
        FullscreenCloseContinuation::IssuedZipListReturn(_)
    ));
    assert!(!app.pending_return_to_parent.is_pending());
    nav
}

#[cfg(windows)]
#[derive(Debug, PartialEq)]
struct ArchiveReaderSnapshot {
    items: Vec<GridItem>,
    items_generation: u64,
    fullscreen: Option<usize>,
    selected: Option<usize>,
    folder: Option<PathBuf>,
    tree_identity: usize,
    display_prefix: Vec<String>,
    raw_prefix: Vec<String>,
    chosen: Option<(Vec<String>, Option<u32>)>,
}

#[cfg(windows)]
fn archive_reader_snapshot(app: &App) -> ArchiveReaderSnapshot {
    let nav = app.zip_nav.as_ref().unwrap();
    ArchiveReaderSnapshot {
        items: app.items.clone(),
        items_generation: app.items_generation,
        fullscreen: app.fullscreen_idx,
        selected: app.selected,
        folder: app.current_folder.clone(),
        tree_identity: Arc::as_ptr(&nav.tree) as usize,
        display_prefix: nav.current().to_vec(),
        raw_prefix: nav.current_raw().to_vec(),
        chosen: nav
            .chosen_list_return_plan()
            .map(|plan| (plan.left_prefix().to_vec(), plan.rows_above())),
    }
}

#[test]
#[cfg(windows)]
fn section1676_taken_terminal_then_real_enter_reopen_keeps_chosen_reader() {
    let (mut app, ctx, _) = archive_return_lifetime_env();
    let nav = take_terminal_archive_return(&mut app);
    let chosen = app
        .zip_nav
        .as_ref()
        .unwrap()
        .chosen_list_return_plan()
        .unwrap();
    app.selected = Some(0);
    ctx.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    assert!(app.handle_keyboard(&ctx).is_none());
    ctx.end_pass().drop_without_applying_deltas();
    settle(&mut app, &ctx);
    assert_eq!(
        app.fullscreen_idx,
        Some(0),
        "Enter reopens a different page in the same installed A level"
    );
    assert!(app.fs_suppress_enter_close_until_release);
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .matches_list_return_plan(&chosen),
        "reopen retains the valid Chosen/tree facts; only old return authority expires"
    );
    let before = archive_reader_snapshot(&app);
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    assert_eq!(
        archive_reader_snapshot(&app),
        before,
        "the taken Terminal must not replace the reopened reader with its parent rows"
    );
    assert!(!app.pending_return_to_parent.is_pending());
}

#[test]
#[cfg(windows)]
fn section1676_taken_terminal_then_real_grid_double_click_reopen_keeps_chosen_reader() {
    let (mut app, _, _) = archive_return_lifetime_env();
    let nav = take_terminal_archive_return(&mut app);
    let chosen = app
        .zip_nav
        .as_ref()
        .unwrap()
        .chosen_list_return_plan()
        .unwrap();
    app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
    app.settings.grid_cols = 4;
    app.settings.thumb_aspect_auto = false;
    app.selected = Some(0);
    app.grid_scroll.set_position(0.0);
    app.grid_scroll.set_reveal_requested(true);
    struct State {
        app: AppTestEnvForTest,
        origin: egui::Pos2,
    }
    let mut configured = false;
    let mut harness = crate::test_ui::builder()
        .with_size(egui::vec2(1000.0, 600.0))
        .with_step_dt(0.01)
        .build_ui_state(
            move |ui, state: &mut State| {
                let ctx = ui.ctx().clone();
                if !configured {
                    crate::ui_fonts::configure_fonts(&ctx);
                    configured = true;
                    ctx.request_repaint();
                    return;
                }
                state.origin = ui.available_rect_before_wrap().min;
                state.app.begin_grid_click_input_frame(&ctx);
                assert!(state.app.render_grid(ui).is_none());
            },
            State {
                app,
                origin: egui::Pos2::ZERO,
            },
        );
    harness.run_steps(6);
    // The first page is another page than the closed reader's page 2. Use the
    // center of its actual painted cell, inside portrait-image letterboxing.
    let pos = harness.state().origin
        + egui::vec2(
            8.0 + harness.state().app.last_cell_size * 0.5,
            8.0 + harness.state().app.last_cell_h * 0.5,
        );
    harness.hover_at(pos);
    for _ in 0..2 {
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        harness.step();
    }
    let ctx = harness.ctx.clone();
    let app = &mut harness.state_mut().app;
    settle(app, &ctx);
    assert_eq!(
        app.fullscreen_idx,
        Some(0),
        "real grid double-click must reopen page 1"
    );
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .matches_list_return_plan(&chosen)
    );
    let before = archive_reader_snapshot(app);
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    assert_eq!(
        archive_reader_snapshot(app),
        before,
        "old Terminal authority must not navigate after the real pointer handler accepted another reader"
    );
    assert!(!app.pending_return_to_parent.is_pending());
}

#[test]
#[cfg(windows)]
fn section1676_new_reader_acceptance_retires_zip_close_before_take() {
    for terminal in [false, true] {
        let (mut app, ctx, _) = archive_return_lifetime_env();
        if terminal {
            let main = app.viewer_context_main();
            app.transfer_terminal_close_return(main, ViewerCloseCause::User);
        } else {
            app.handle_fullscreen_close_request();
        }
        app.close_fullscreen_now();
        assert!(
            app.pending_return_to_parent.is_pending(),
            "the old close has not been taken; terminal={terminal}"
        );
        app.open_fullscreen(0, HistoryTrigger::UserChosen);
        settle(&mut app, &ctx);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(!app.pending_return_to_parent.is_pending());
        assert!(
            app.take_pending_return_to_parent_nav().is_none(),
            "new reader acceptance retires untaken ZIP return; terminal={terminal}"
        );
        assert!(
            app.zip_nav
                .as_ref()
                .unwrap()
                .chosen_list_return_plan()
                .is_some()
        );
    }
}

#[test]
#[cfg(windows)]
fn section1676_repeated_take_retains_the_issued_zip_authority() {
    let (mut app, ctx, _) = archive_return_lifetime_env();
    let nav = take_terminal_archive_return(&mut app);
    let ticket = match &app.pending_return_to_parent {
        FullscreenCloseContinuation::IssuedZipListReturn(owner) => owner.ticket(),
        _ => panic!("issued owner must remain in the original slot"),
    };
    for _ in 0..3 {
        assert!(app.take_pending_return_to_parent_nav().is_none());
        assert!(!app.pending_return_to_parent.is_pending());
        assert!(
            matches!(&app.pending_return_to_parent, FullscreenCloseContinuation::IssuedZipListReturn(current) if current.accepts(&ticket))
        );
    }
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    settle(&mut app, &ctx);
    assert_returned(&app, &[], "A/");
}

#[test]
#[cfg(windows)]
fn section1676_refused_reader_open_keeps_issued_zip_authority() {
    let (mut app, ctx, _) = archive_return_lifetime_env();
    let nav = take_terminal_archive_return(&mut app);
    let before = archive_reader_snapshot(&app);
    let invalid = app.items.len();
    app.open_fullscreen(invalid, HistoryTrigger::UserChosen);
    assert_eq!(archive_reader_snapshot(&app), before);
    assert!(matches!(
        app.pending_return_to_parent,
        FullscreenCloseContinuation::IssuedZipListReturn(_)
    ));
    assert!(app.take_pending_return_to_parent_nav().is_none());
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    settle(&mut app, &ctx);
    assert_returned(&app, &[], "A/");
}

#[test]
#[cfg(windows)]
fn section1676_issued_zip_return_installs_its_exact_destination_once() {
    let (mut app, ctx, _) = archive_return_lifetime_env();
    let nav = take_terminal_archive_return(&mut app);
    let duplicate = match &nav {
        AddressBarNav::ZipListReturn(request) => AddressBarNav::ZipListReturn(request.clone()),
        _ => panic!("terminal fixture must issue a ZIP return"),
    };
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    settle(&mut app, &ctx);
    assert_returned(&app, &[], "A/");
    let before = archive_reader_snapshot(&app);
    assert!(app.apply_fullscreen_close_nav_immediate(duplicate));
    assert_eq!(archive_reader_snapshot(&app), before);
    assert!(app.take_pending_return_to_parent_nav().is_none());
}

#[test]
#[cfg(windows)]
fn section1676_sibling_reader_acceptance_keeps_main_issued_zip_authority() {
    let (mut app, ctx, book) = archive_return_lifetime_env();
    let nav = take_terminal_archive_return(&mut app);
    let ticket = match &app.pending_return_to_parent {
        FullscreenCloseContinuation::IssuedZipListReturn(owner) => owner.ticket(),
        _ => panic!("main issued owner"),
    };
    let before = archive_reader_snapshot(&app);
    app.settings.detached_viewer_open_images_in_window = true;
    ctx.set_embed_viewports(false);
    assert!(app.start_active_detached_book_context(
        ViewerContextDescriptor::Zip {
            path: book,
            entry_name: Some("B/001.png".into()),
            archive_source_override: None,
        },
        &ctx,
        None,
        None
    ));
    let sibling = app.active_viewer_context_id().unwrap();
    assert_ne!(sibling, app.viewer_context_main());
    app.with_active_viewer_context(|viewer| {
        settle(viewer, &ctx);
        assert!(viewer.fullscreen_idx.is_some());
        assert_eq!(
            viewer.viewer_presentation,
            ViewerPresentation::DetachedWindow
        );
    })
    .unwrap();
    assert_eq!(archive_reader_snapshot(&app), before);
    assert!(
        matches!(&app.pending_return_to_parent, FullscreenCloseContinuation::IssuedZipListReturn(current) if current.accepts(&ticket))
    );
    assert!(app.take_pending_return_to_parent_nav().is_none());
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    settle(&mut app, &ctx);
    assert_returned(&app, &[], "A/");
    assert_eq!(app.active_viewer_context_id(), Some(sibling));
    assert!(
        app.with_viewer_context_state(sibling, |viewer| viewer.fullscreen_idx.is_some())
            .unwrap()
    );
}

#[test]
#[cfg(windows)]
fn section1676_issued_live_terminal_teardown_preserves_valid_chosen_bookmark_owner() {
    use crate::bookmark_browser::BookmarkViewReturnTarget;
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("outer.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    app.bookmark_view_state = Some(BookmarkViewState::Opening {
        target: BookmarkViewReturnTarget::Book(book.clone()),
        grid: BookmarkViewReturnGridState {
            row_keys: vec![(1, 4)],
            selected_key: Some((1, 4)),
            opened_key: Some((1, 4)),
            scroll_offset_y: 123.0,
        },
    });
    app.settings.auto_fullscreen_zip_pdf = true;
    activate(&mut app, &ctx, "A/", true);
    app.handle_fullscreen_close_request();
    let nav = app
        .take_pending_return_to_parent_nav()
        .expect("accepted Live issues its navigation");
    assert!(matches!(
        &nav,
        AddressBarNav::ZipListReturn(ZipListReturnRequest::Live { .. })
    ));
    assert!(matches!(
        app.pending_return_to_parent,
        FullscreenCloseContinuation::IssuedZipListReturn(_)
    ));
    assert!(
        app.zip_nav
            .as_ref()
            .unwrap()
            .chosen_list_return_plan()
            .is_some()
    );
    let main = app.viewer_context_main();
    app.transfer_terminal_close_return(main, ViewerCloseCause::User);
    app.close_fullscreen_now();
    assert!(app.fullscreen_idx.is_none());
    assert!(
        matches!(&app.bookmark_view_state, Some(BookmarkViewState::Opening { target: BookmarkViewReturnTarget::Book(path), grid }) if path == &book && grid.scroll_offset_y == 123.0),
        "terminal teardown must derive ChosenZipList from the still-issued Live owner"
    );
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    settle(&mut app, &ctx);
    assert_returned(&app, &[], "A/");
    assert!(
        matches!(&app.bookmark_view_state, Some(BookmarkViewState::Opening { target: BookmarkViewReturnTarget::Book(path), grid }) if path == &book && grid.scroll_offset_y == 123.0)
    );
    assert!(matches!(
        app.handle_grid_parent_folder_action(),
        Some(AddressBarNav::Bookmarks)
    ));
}

#[test]
#[cfg(windows)]
fn section1676_dropped_issued_navigation_allows_a_fresh_user_close() {
    let (mut app, ctx, _) = archive_return_lifetime_env();
    app.handle_fullscreen_close_request();
    {
        let nav = app
            .take_pending_return_to_parent_nav()
            .expect("first Close issues its navigation");
        assert!(matches!(
            &nav,
            AddressBarNav::ZipListReturn(ZipListReturnRequest::Live { .. })
        ));
        // Keep no weak probe or cloned request: this is the actual lifetime
        // where all delivery owners disappear before reaching the consumer.
        drop(nav);
    }
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(matches!(
        app.pending_return_to_parent,
        FullscreenCloseContinuation::IssuedZipListReturn(_)
    ));
    app.handle_fullscreen_close_request();
    assert!(
        matches!(
            app.pending_return_to_parent,
            FullscreenCloseContinuation::LiveZipListReturn(_)
        ),
        "an orphaned issued slot must not permanently block a later user Close"
    );
    let nav = app
        .take_pending_return_to_parent_nav()
        .expect("fresh Close must issue a deliverable navigation");
    assert!(matches!(
        &nav,
        AddressBarNav::ZipListReturn(ZipListReturnRequest::Live { .. })
    ));
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    settle(&mut app, &ctx);
    assert_returned(&app, &[], "A/");
}
