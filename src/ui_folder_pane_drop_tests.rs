use super::*;
use std::path::PathBuf;

struct TreeDropTest {
    ctx: egui::Context,
    app: crate::app::AppTestEnvForTest,
    root: PathBuf,
    target: PathBuf,
    queries: usize,
    files_available: bool,
    invoked: Vec<PathBuf>,
    invocation_rx: std::sync::mpsc::Receiver<PathBuf>,
    navigation: Option<FolderPaneActivation>,
}

impl TreeDropTest {
    fn new() -> Self {
        let mut app = crate::app::setup_app_for_test();
        app.indexer_init.mark_unavailable_if_not_ready();
        app.main_hwnd = Some(1);
        app.settings.folder_tree_pane_visible = true;
        let root = app.tmp.path().join("tree");
        let target = root.join("target");
        std::fs::create_dir_all(&target).unwrap();
        app.settings.book_root = Some(app.tmp.path().join("books"));
        let (invocation_tx, invocation_rx) = std::sync::mpsc::channel();
        app.shell_paste_service =
            crate::shell_paste::ShellPasteService::with_test_invoker(move |_, folder| {
                invocation_tx.send(folder.to_owned()).unwrap();
                Ok(())
            });
        app.current_folder = Some(root.clone());
        app.folder_pane = folder_pane::FolderPaneState::test_tree(&root, vec![target.clone()]);
        app.folder_pane
            .test_seed_listing(&target, vec![target.join("nested")], false);
        app.folder_pane.set_cursor(root.clone());
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        crate::ui_fonts::configure_fonts(&ctx);
        let mut test = Self {
            ctx,
            app,
            root,
            target,
            queries: 0,
            files_available: true,
            invoked: Vec::new(),
            invocation_rx,
            navigation: None,
        };
        test.frame(vec![]);
        test.frame(vec![]);
        test
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        self.frame_with_keyboard_route(events, false);
    }

    fn keyboard_frame(&mut self, events: Vec<egui::Event>) {
        self.frame_with_keyboard_route(events, true);
    }

    fn frame_with_keyboard_route(&mut self, events: Vec<egui::Event>, route_keyboard: bool) {
        let ctx = self.ctx.clone();
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(480.0, 300.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let Self {
                    app,
                    queries,
                    files_available,
                    navigation,
                    ..
                } = self;
                let mut render_tree = |app: &mut App| {
                    *navigation = app.render_folder_pane_tree_with(
                        ui,
                        || {
                            *queries += 1;
                            *files_available
                        },
                        |app, ctx, path| {
                            app.paste_tree_files_to_folder(ctx, path);
                        },
                    );
                };
                if route_keyboard {
                    assert!(
                        app.folder_tree_keyboard_frame_for_test(&ctx, render_tree)
                            .is_none(),
                        "the menu must not navigate the background grid"
                    );
                } else {
                    render_tree(app);
                }
            },
        )
        .drop_without_applying_deltas();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while self.app.shell_paste_request.is_some() {
            self.app.poll_shell_paste(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "fake tree paste did not finish"
            );
            std::thread::yield_now();
        }
        self.invoked.extend(self.invocation_rx.try_iter());
    }

    fn row(&self) -> egui::Rect {
        self.app
            .folder_pane
            .label_display
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .find(|row| row.path == self.target)
            .unwrap()
            .row_rect
    }

    fn click(&mut self, pos: egui::Pos2, button: egui::PointerButton) {
        self.frame(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        self.frame(vec![egui::Event::PointerButton {
            pos,
            button,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
    }

    fn menu_button(&self) -> (egui::Rect, bool) {
        self.ctx
            .data(|data| {
                data.get_temp::<(egui::Rect, bool)>(egui::Id::unique(
                    "folder_pane_test_paste_button",
                ))
            })
            .unwrap()
    }
}

#[test]
fn folder_tree_drop_secondary_click_keeps_navigation_cursor_and_selection_and_pastes_to_captured_row()
 {
    let mut test = TreeDropTest::new();
    test.app.selected = Some(7);
    let row = test.row();
    test.click(row.center(), egui::PointerButton::Secondary);
    assert!(test.navigation.is_none());
    assert_eq!(test.app.selected, Some(7));
    assert_eq!(test.app.current_folder.as_ref(), Some(&test.root));
    assert!(
        test.app
            .folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == test.root && row.is_cursor)
    );
    assert_eq!(test.queries, 1);
    for _ in 0..3 {
        test.frame(vec![]);
    }
    assert_eq!(
        test.queries, 1,
        "clipboard availability belongs to this menu opening"
    );
    let captured = test
        .ctx
        .data(|data| data.get_temp::<FolderPanePasteMenu>(folder_pane_paste_menu_id()))
        .unwrap();
    assert_eq!(captured.path, test.target);
    let (button, enabled) = test.menu_button();
    assert!(enabled);
    test.click(button.center(), egui::PointerButton::Primary);
    assert_eq!(test.invoked, vec![test.target.clone()]);
    assert!(
        test.app.post_operation_selection.is_none(),
        "another folder must not reserve selection in this list"
    );
}

#[test]
fn folder_tree_drop_paste_disabled_reasons_and_wheel_do_not_leak() {
    assert!(folder_pane_paste_enabled(true, Ok(())).is_ok());
    assert!(
        folder_pane_paste_enabled(false, Ok(()))
            .unwrap_err()
            .contains("ファイル")
    );
    assert_eq!(
        folder_pane_paste_enabled(true, Err("blocked".into())),
        Err("blocked".into())
    );
    let mut test = TreeDropTest::new();
    test.files_available = false;
    test.click(test.row().center(), egui::PointerButton::Secondary);
    let (button, enabled) = test.menu_button();
    assert!(!enabled);
    test.frame(vec![egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -50.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
    }]);
    assert!(!test.ctx.input(|input| {
        input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::MouseWheel { .. }))
    }));
    test.click(button.center(), egui::PointerButton::Primary);
    assert!(test.invoked.is_empty());
    assert_eq!(test.queries, 1);
}

#[test]
fn folder_tree_drop_menu_rechecks_admission_before_paste() {
    let mut test = TreeDropTest::new();
    test.click(test.row().center(), egui::PointerButton::Secondary);
    let (button, enabled) = test.menu_button();
    assert!(enabled);
    test.app
        .shutdown_requested
        .store(true, std::sync::atomic::Ordering::SeqCst);
    test.click(button.center(), egui::PointerButton::Primary);
    assert!(test.invoked.is_empty());
    assert_eq!(test.queries, 1);
}

#[test]
fn folder_tree_drop_books_menu_stays_enabled_until_operation_time_refusal() {
    let mut test = TreeDropTest::new();
    test.app.settings.book_root = Some(test.target.clone());
    let revision = test.app.book_append_undo_revision;
    let current = test.root.clone();
    test.app
        .request_post_operation_selection_for_added_items(current);
    crate::app::main_drop::take_books_root_resolutions_for_test();
    test.click(test.row().center(), egui::PointerButton::Secondary);
    for _ in 0..3 {
        test.frame(vec![]);
    }
    assert_eq!(
        crate::app::main_drop::take_books_root_resolutions_for_test(),
        0,
        "drawing and menu eligibility never resolve the bookshelf root"
    );
    let (button, enabled) = test.menu_button();
    assert!(
        enabled,
        "bookshelf refusal belongs to the operation boundary"
    );
    test.click(button.center(), egui::PointerButton::Primary);
    assert_eq!(
        crate::app::main_drop::take_books_root_resolutions_for_test(),
        1
    );
    assert!(test.invoked.is_empty());
    assert_eq!(test.app.book_append_undo_revision, revision);
    assert!(test.app.post_operation_selection.is_some());
    assert_eq!(
        test.app.fs_feedback_toast.as_ref().unwrap().0,
        "本棚の中へは移動・コピーできません"
    );
}

#[test]
fn folder_tree_drop_menu_is_retired_when_chrome_or_tree_is_hidden_or_rows_disappear() {
    for exit in 0..3 {
        let mut test = TreeDropTest::new();
        test.click(test.row().center(), egui::PointerButton::Secondary);
        assert!(egui::Popup::is_id_open(
            &test.ctx,
            folder_pane_paste_menu_id()
        ));
        match exit {
            0 => test.app.finish_folder_pane_presentation(&test.ctx, false),
            1 => {
                test.app.settings.folder_tree_pane_visible = false;
                test.app.finish_folder_pane_presentation(&test.ctx, true);
            }
            _ => {
                // Default discovers the real machine's drives, so it still has rows.
                test.app.folder_pane.test_set_drive_choices(vec![]);
                test.app.folder_pane.test_set_default_drive(None);
                assert!(test.app.folder_pane.visible_rows().is_empty());
                test.frame(vec![]);
            }
        }
        assert!(
            !egui::Popup::is_id_open(&test.ctx, folder_pane_paste_menu_id()),
            "menu must retire for exit {exit}"
        );
        assert!(
            test.ctx
                .data(|data| data.get_temp::<FolderPanePasteMenu>(folder_pane_paste_menu_id()))
                .is_none()
        );
        assert!(test.invoked.is_empty());
    }
}

#[test]
fn folder_tree_drop_row_includes_arrow_label_and_trailing_area_but_excludes_leading_indent() {
    let test = TreeDropTest::new();
    let display = test.app.folder_pane.label_display.as_ref().unwrap();
    let row = display
        .rows
        .iter()
        .find(|row| row.path == test.target)
        .unwrap();
    assert!(
        row.row_rect.left() > display.rect.left(),
        "nested leading indentation stays non-target"
    );
    assert!(row.row_rect.contains(row.label_rect.left_center()));
    assert!(row.row_rect.contains(row.label_rect.right_center()));
    assert!(row.row_rect.right() >= row.label_rect.right());
    assert!(row.arrow_rect.is_positive());
    assert!(row.row_rect.contains_rect(row.arrow_rect));
    assert!(!row.row_rect.contains(egui::pos2(
        display.rect.left() + 1.0,
        row.row_rect.center().y
    )));
}

#[test]
fn folder_tree_drop_visible_empty_pane_keeps_header_and_blank_area_out_of_grid_route() {
    let mut app = crate::app::setup_app_for_test();
    app.indexer_init.mark_unavailable_if_not_ready();
    app.settings.folder_tree_pane_visible = true;
    app.folder_pane.test_set_drive_choices(vec![]);
    app.folder_pane.test_set_default_drive(None);
    let ctx = egui::Context::default();
    let metrics = crate::main_drop_target::WindowMetrics {
        client_size_px: [1000, 600],
        dpi: 96,
    };
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            ..Default::default()
        },
        |ui| {
            let (_, rect) = app.render_folder_pane(ui);
            let geometry = app.main_drop_geometry(ui.ctx(), metrics);
            assert_eq!(geometry.pane_rect, rect.unwrap());
            assert!(geometry.pane_rect.is_positive());
            assert!(geometry.rows.is_empty());
        },
    )
    .drop_without_applying_deltas();
}

#[test]
fn folder_tree_drop_two_pass_release_keeps_the_first_menu_path_and_clipboard_query() {
    let mut test = TreeDropTest::new();
    let position = test.row().center();
    let mut pass_count = 0;
    let ctx = test.ctx.clone();
    ctx.options_mut(|options| options.max_passes = 2.try_into().unwrap());
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(480.0, 300.0),
            )),
            events: vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Secondary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Secondary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        },
        |ui| {
            let TreeDropTest {
                app,
                queries,
                root,
                target,
                ..
            } = &mut test;
            app.render_folder_pane_tree_with(
                ui,
                || {
                    *queries += 1;
                    true
                },
                |_, _, _| panic!("unexpected paste"),
            );
            if pass_count == 0 {
                app.folder_pane.test_seed_listing(
                    root,
                    vec![root.join("new-first-row"), target.clone()],
                    true,
                );
                ui.ctx()
                    .request_discard("tree layout changed after the secondary release");
            }
            pass_count += 1;
        },
    )
    .drop_without_applying_deltas();
    assert_eq!(pass_count, 2);
    assert_eq!(test.queries, 1);
    assert_eq!(
        ctx.data(|data| data.get_temp::<FolderPanePasteMenu>(folder_pane_paste_menu_id()))
            .unwrap()
            .path,
        test.target
    );
}

#[test]
fn folder_tree_drop_disabled_pane_retires_the_menu_but_keeps_visible_panel_geometry() {
    let mut test = TreeDropTest::new();
    test.click(test.row().center(), egui::PointerButton::Secondary);
    let image = test.root.join("image.jpg");
    crate::app::seed_snapshot_context_for_test(
        &mut test.app,
        &test.root.to_string_lossy(),
        &[&image.to_string_lossy()],
    );
    test.app
        .activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    assert!(test.app.folder_pane_disabled());
    let ctx = test.ctx.clone();
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            ..Default::default()
        },
        |ui| {
            let (_, panel) = test.app.render_folder_pane(ui);
            test.app.finish_folder_pane_presentation(ui.ctx(), true);
            let geometry = test.app.main_drop_geometry(
                ui.ctx(),
                crate::main_drop_target::WindowMetrics {
                    client_size_px: [1000, 600],
                    dpi: 96,
                },
            );
            assert_eq!(geometry.pane_rect, panel.unwrap());
            assert!(test.app.shell_transfer_admission(ui.ctx()).is_err());
        },
    )
    .drop_without_applying_deltas();
    assert!(!egui::Popup::is_id_open(&ctx, folder_pane_paste_menu_id()));
    assert!(
        ctx.data(|data| data.get_temp::<FolderPanePasteMenu>(folder_pane_paste_menu_id()))
            .is_none()
    );
}

fn menu_key_events(key: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    [true, false]
        .into_iter()
        .map(|pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        })
        .collect()
}

fn seed_menu_grid(test: &mut TreeDropTest) {
    let first = test.root.join("first.jpg");
    let second = test.root.join("second.jpg");
    crate::app::seed_snapshot_context_for_test(
        &mut test.app,
        &test.root.to_string_lossy(),
        &[&first.to_string_lossy(), &second.to_string_lossy()],
    );
}

#[test]
fn folder_tree_drop_menu_escape_uses_normal_keyboard_route_without_changing_background_focus() {
    for tree_focused in [false, true] {
        let mut test = TreeDropTest::new();
        seed_menu_grid(&mut test);
        if tree_focused {
            test.app.folder_pane.set_focus_tree();
        } else {
            test.app.folder_pane.set_focus_grid();
        }
        test.click(test.row().center(), egui::PointerButton::Secondary);
        assert!(egui::Popup::is_id_open(
            &test.ctx,
            folder_pane_paste_menu_id()
        ));
        test.keyboard_frame(menu_key_events(egui::Key::Escape, egui::Modifiers::NONE));
        assert!(!egui::Popup::is_id_open(
            &test.ctx,
            folder_pane_paste_menu_id()
        ));
        assert_eq!(test.app.folder_pane.has_focus, tree_focused);
        assert_eq!(test.app.current_folder.as_ref(), Some(&test.root));
        assert_eq!(test.app.selected, Some(0));
        assert!(test.app.checked.is_empty());
        assert!(test.invoked.is_empty());
        if !tree_focused {
            // The next Escape belongs to the normal tree-focus command again.
            test.keyboard_frame(menu_key_events(egui::Key::Escape, egui::Modifiers::NONE));
            assert!(test.app.folder_pane.has_focus);
        }
    }
}

#[test]
fn folder_tree_drop_menu_arrows_and_enter_use_normal_keyboard_route_without_opening_tree_folder() {
    let mut test = TreeDropTest::new();
    seed_menu_grid(&mut test);
    test.app.folder_pane.set_focus_tree();
    test.click(test.row().center(), egui::PointerButton::Secondary);
    for key in [
        egui::Key::ArrowRight,
        egui::Key::ArrowLeft,
        egui::Key::ArrowUp,
        egui::Key::ArrowDown,
    ] {
        test.keyboard_frame(menu_key_events(key, egui::Modifiers::NONE));
        assert!(egui::Popup::is_id_open(
            &test.ctx,
            folder_pane_paste_menu_id()
        ));
        assert!(
            test.app
                .folder_pane
                .visible_rows()
                .iter()
                .any(|row| row.path == test.root && row.is_cursor)
        );
        assert_eq!(test.app.current_folder.as_ref(), Some(&test.root));
        assert_eq!(test.app.selected, Some(0));
        assert!(test.invoked.is_empty());
    }
    // egui's menu navigation requests the item focus for the next pass.
    test.keyboard_frame(vec![]);
    let focused = test
        .ctx
        .memory(|memory| memory.focused())
        .expect("paste menu item focus");
    assert_eq!(
        test.ctx.read_response(focused).unwrap().rect,
        test.menu_button().0
    );
    test.keyboard_frame(menu_key_events(egui::Key::Enter, egui::Modifiers::NONE));
    assert_eq!(test.invoked, vec![test.target.clone()]);
    assert_eq!(test.app.current_folder.as_ref(), Some(&test.root));
    assert_eq!(test.app.selected, Some(0));
    assert!(!egui::Popup::is_id_open(
        &test.ctx,
        folder_pane_paste_menu_id()
    ));
}

#[test]
fn folder_tree_drop_menu_blocks_grid_shortcut_phase_and_resumes_after_escape() {
    for tree_focused in [false, true] {
        let mut test = TreeDropTest::new();
        seed_menu_grid(&mut test);
        if tree_focused {
            test.app.folder_pane.set_focus_tree();
        } else {
            test.app.folder_pane.set_focus_grid();
        }
        test.click(test.row().center(), egui::PointerButton::Secondary);
        for (key, modifiers) in [
            (egui::Key::A, egui::Modifiers::CTRL),
            (egui::Key::O, egui::Modifiers::CTRL),
            (egui::Key::F, egui::Modifiers::NONE),
        ] {
            test.keyboard_frame(menu_key_events(key, modifiers));
            assert!(
                test.app.checked.is_empty(),
                "{key:?} must not select grid items"
            );
            assert!(!test.app.show_open_folder_dialog);
            assert!(test.app.settings.folder_tree_pane_visible);
            assert_eq!(test.app.folder_pane.has_focus, tree_focused);
            assert!(egui::Popup::is_id_open(
                &test.ctx,
                folder_pane_paste_menu_id()
            ));
        }
        test.keyboard_frame(menu_key_events(egui::Key::Escape, egui::Modifiers::NONE));
        test.keyboard_frame(menu_key_events(egui::Key::A, egui::Modifiers::CTRL));
        assert_eq!(
            test.app.checked.len(),
            2,
            "grid shortcuts resume after the menu closes"
        );
    }
}
