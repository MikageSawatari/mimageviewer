//! §1.920: the linked viewer's edit draft excludes grid-originated item input.

#![cfg(windows)]

use super::*;
use crate::filename_stack_ui::{DETACHED_VIEWER_EDITING_GRID_NOTICE, GridItemInputBlock};

fn linked_editing_app() -> AppTestEnvForTest {
    let mut app = setup_app_for_test();
    let root = app.tmp.path().to_owned();
    app.items = vec![
        GridItem::Image(root.join("old.png")),
        GridItem::Image(root.join("new.png")),
    ];
    app.thumbnails = vec![crate::grid_item::ThumbnailState::Failed; 2];
    app.image_metas = vec![Some((16, 16)); 2];
    app.visible_indices = vec![0, 1];
    app.details_order = vec![0, 1];
    app.current_folder = Some(root);
    app.items_are_drive_list = false;
    app.fullscreen_idx = Some(0);
    app.selected = Some(0);
    app.main_hwnd = Some(1);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.detached_viewer_independent_active = false;
    app.conceal_mode = true;
    app
}

fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn notice_count(app: &App) -> usize {
    app.feedback_toasts_for_test
        .iter()
        .filter(|(message, _)| message == DETACHED_VIEWER_EDITING_GRID_NOTICE)
        .count()
}

#[test]
fn page_edit_920_grid_gate_covers_all_six_tools_and_stays_read_only() {
    let mut app = linked_editing_app();
    for tool in 0..6 {
        app.conceal_mode = tool == 0;
        app.erase_mode = tool == 1;
        app.text_mode = tool == 2;
        app.local_adjust_mode = tool == 3;
        app.export_crop_mode = tool == 4;
        app.sns_split = (tool == 5).then(|| {
            crate::sns_split::SnsSplitLayout::centered_max(
                crate::sns_split::SnsTarget::X,
                2,
                [160, 160],
            )
        });
        for _ in 0..3 {
            assert_eq!(
                app.grid_item_input_block(),
                Some(GridItemInputBlock::DetachedViewerEditing)
            );
            assert!(!app.grid_item_input_allowed());
        }
        assert!(!GridItemInputBlock::DetachedViewerEditing.item_independent_controls_allowed());
        assert_eq!(notice_count(&app), 0, "the getter cannot notify");
    }
    app.sns_split = None;
    assert!(app.grid_item_input_allowed());
    app.conceal_mode = true;
    for presentation in [
        ViewerPresentation::MainWindow,
        ViewerPresentation::Fullscreen,
    ] {
        app.viewer_presentation = presentation;
        assert!(app.grid_item_input_allowed());
    }
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.detached_viewer_independent_active = true;
    assert!(app.grid_item_input_allowed());
    app.detached_viewer_independent_active = false;
    app.fullscreen_idx = None;
    assert!(app.grid_item_input_allowed());
}

#[test]
fn page_edit_920_inner_preserves_existing_grid_refusal_when_detached_editing_overlaps() {
    for block in [
        GridItemInputBlock::DirectNavigationScan,
        GridItemInputBlock::StackRefresh,
    ] {
        let mut app = linked_editing_app();
        app.conceal_mask_size = [16, 16];
        app.conceal_mask = Some(vec![true; 16 * 16]);
        let old_key = app.page_path_key(0).unwrap();
        let items = app.items.clone();
        let generation = app.items_generation;
        let (held_scan, rx) = std::sync::mpsc::channel();
        match block {
            GridItemInputBlock::DirectNavigationScan => {
                let path = app.tmp.path().to_path_buf();
                app.folder_pane_open_pending = Some(FolderPaneOpenPending {
                    started_at: std::time::Instant::now(),
                    path: path.clone(),
                    cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    rx,
                    purpose: FolderOpenScanPurpose::DirectNavigation(Box::new(
                        DirectNavigationContinuation {
                            path,
                            owner: OpenRequestOwner::Navigation,
                            grid_effects: None,
                            classified_kind: Some(OpenPathKind::Directory),
                            restore_intent: StartupListIntent::ExplicitList,
                            navigation: None,
                            stage: MainListRequestStage::Admission,
                            search_origin: None,
                            tag_admission: None,
                            auto_fullscreen: false,
                            rescans: 0,
                        },
                    )),
                    epub_restore: None,
                    navigation: None,
                });
            }
            GridItemInputBlock::StackRefresh => {
                let view = Arc::new(crate::filename_stack::StackView::build(
                    app.tmp.path().to_path_buf(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    app.settings.stack_separator,
                    app.settings.sort_order,
                ));
                app.stack_view = Some(Arc::clone(&view));
                app.stack_return_state =
                    Some(crate::filename_stack_ui::StackReturnState::Refreshing { view });
            }
            _ => unreachable!(),
        }
        assert!(app.detached_viewer_editing_blocks_grid_input());
        assert_eq!(app.grid_item_input_block(), Some(block));
        let admission = app.open_fullscreen_with_metadata_boundary_inner(
            1,
            HistoryTrigger::UserChosen,
            FsOpenMaterialization::Eager,
            FsPageLoadContract::Sequential,
            None,
            FsMetadataBoundary::Manual,
            None,
            ViewerOpenAdoption::ContextContinuation,
            None,
        );
        assert_eq!(admission, ActionAdmission::Rejected, "{block:?}");
        assert_eq!(app.grid_item_input_block(), Some(block));
        assert!(!app.grid_item_input_allowed());
        assert!(
            app.conceal_mode,
            "a refused open must not finish the old draft"
        );
        assert_eq!(app.conceal_mask_size, [16, 16]);
        assert_eq!(
            app.conceal_mask.as_deref(),
            Some([true; 16 * 16].as_slice())
        );
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.items, items);
        assert_eq!(app.items_generation, generation);
        assert!(
            app.conceal_db
                .as_ref()
                .unwrap()
                .get_full_checked(&old_key)
                .unwrap()
                .is_none()
        );
        if block == GridItemInputBlock::DirectNavigationScan {
            assert!(
                !app.folder_pane_open_pending
                    .as_ref()
                    .unwrap()
                    .cancel
                    .load(std::sync::atomic::Ordering::Relaxed)
            );
            app.folder_pane_open_pending = None;
        } else {
            app.stack_return_state = None;
        }
        assert_eq!(
            app.grid_item_input_block(),
            Some(GridItemInputBlock::DetachedViewerEditing)
        );
        assert_eq!(notice_count(&app), 0);
        drop(held_scan);
    }
}

#[cfg(windows)]
#[test]
fn page_edit_920_grid_gate_does_not_read_foreground_draft_as_sibling_editing() {
    let mut app = linked_editing_app();
    let sibling = app.build_window_context_for_test(1920, |owner| {
        owner.items = vec![GridItem::Image(PathBuf::from("sibling.png"))];
        owner.fullscreen_idx = Some(0);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        owner.detached_viewer_independent_active = false;
    });
    let items = app.items.clone();
    let generation = app.items_generation;
    app.with_viewer_context(sibling, |owner| {
        assert!(
            owner.conceal_mode,
            "the App draft is shared but belongs to main"
        );
        assert!(owner.grid_item_input_allowed());
        assert!(!owner.detached_viewer_editing_blocks_grid_input());
    })
    .unwrap();
    assert_eq!(app.items, items);
    assert_eq!(app.items_generation, generation);
    assert!(app.conceal_mode);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(
        app.grid_item_input_block(),
        Some(GridItemInputBlock::DetachedViewerEditing)
    );
    assert_eq!(notice_count(&app), 0);
}

#[cfg(windows)]
#[test]
fn page_edit_920_main_keyboard_rejects_enter_delete_arrows_escape_and_refresh_allowlist() {
    let _foreground = crate::test_native_focus::scoped(1, 1, true);
    let mut app = linked_editing_app();
    let ctx = egui::Context::default();
    for action in [
        KeyAction::GridOpenPreferences,
        KeyAction::GridReload,
        KeyAction::GridToggleStackMode,
        KeyAction::GridConvertEpubToPdf,
        KeyAction::GridOpenLocationDriveList,
        KeyAction::GridOpenBook1,
    ] {
        app.keymap = crate::keymap::Keymap::from_ini_str(&format!(
            "[{}]\n{} = F13\n",
            action.context().ini_name(),
            action.ini_name(),
        ));
        ctx.run_pass(
            egui::RawInput {
                events: vec![key(egui::Key::F13)],
                ..Default::default()
            },
            |ctx| {
                app.input_update_begin(ctx);
                assert!(app.handle_keyboard(ctx).is_none());
                assert!(app.input_main_grid_shortcuts(ctx, false).is_none());
                assert!(
                    app.handle_stack_refresh_keyboard(
                        ctx,
                        true,
                        Default::default(),
                        Default::default(),
                        GridItemInputBlock::DetachedViewerEditing
                    )
                    .is_none()
                );
            },
        )
        .drop_without_applying_deltas();
        assert!(!app.show_preferences, "{action:?}");
        assert!(!app.stack_mode_requested, "{action:?}");
        assert_eq!(app.items.len(), 2, "{action:?}");
        assert!(
            app.saved_group_open_request_id_for_test().is_none(),
            "{action:?}"
        );
    }
    app.keymap = crate::keymap::Keymap::default();
    for key_value in [
        egui::Key::Enter,
        egui::Key::Delete,
        egui::Key::ArrowRight,
        egui::Key::Escape,
    ] {
        let before = notice_count(&app);
        ctx.run_pass(
            egui::RawInput {
                events: vec![key(key_value)],
                ..Default::default()
            },
            |ctx| {
                app.input_update_begin(ctx);
                assert!(app.handle_keyboard(ctx).is_none());
                app.handle_delete_key(ctx);
                app.notify_detached_viewer_editing_grid_key_input(ctx);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(notice_count(&app), before + 1, "{key_value:?}");
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.items.len(), 2);
        assert!(!app.show_delete_confirm);
        assert!(app.conceal_mode);
        assert!(!app.stack_mode_requested);
        assert_eq!(
            app.fs_feedback_toast_surface,
            Some(ActionSurface::MainWindow)
        );
    }
}

#[test]
fn page_edit_920_rejected_key_notice_is_one_per_input_frame_and_silent_on_repeat() {
    let mut app = linked_editing_app();
    let ctx = egui::Context::default();
    for repeat in [false, true] {
        let mut event = key(egui::Key::Enter);
        if let egui::Event::Key { repeat: value, .. } = &mut event {
            *value = repeat;
        }
        ctx.run_pass(
            egui::RawInput {
                events: vec![event],
                ..Default::default()
            },
            |ctx| {
                app.notify_detached_viewer_editing_grid_key_input(ctx);
                app.notify_detached_viewer_editing_grid_key_input(ctx);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(notice_count(&app), 1);
    }
    ctx.run_pass(Default::default(), |ctx| {
        app.notify_detached_viewer_editing_grid_key_input(ctx);
    })
    .drop_without_applying_deltas();
    assert_eq!(notice_count(&app), 1);
}

#[test]
fn page_edit_920_key_notice_preserves_text_and_ime_escape_ownership() {
    let mut app = linked_editing_app();
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    let field = egui::Id::unique("page_edit_920_text_field");
    let mut text = "draft".to_owned();
    let mut focus = true;
    ctx.run_ui(Default::default(), |ui| {
        crate::ime_focus::show_singleline(ui, &mut text, Some(&mut focus), |edit| edit.id(field));
    })
    .drop_without_applying_deltas();
    for composing in [false, true] {
        let mut events = vec![key(egui::Key::Escape)];
        if composing {
            events.insert(
                0,
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "あ".into(),
                    active_range_chars: None,
                }),
            );
        }
        ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                assert!(app.shortcuts_blocked_by_text_input(ui.ctx()));
                app.notify_detached_viewer_editing_grid_key_input(ui.ctx());
                assert!(ui.ctx().input(|input| input.key_pressed(egui::Key::Escape)));
                crate::ime_focus::show_singleline(ui, &mut text, Some(&mut focus), |edit| {
                    edit.id(field)
                });
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(notice_count(&app), 0);
        assert!(app.conceal_mode);
    }
}

#[test]
fn page_edit_920_disabled_grid_pointer_rejects_selection_and_notifies_only_on_press() {
    for view in [
        crate::settings::GridViewMode::Thumbnail,
        crate::settings::GridViewMode::Details,
    ] {
        let mut app = linked_editing_app();
        app.settings.grid_view_mode = view;
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let mut draw = |events| {
            ctx.run_pass(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640., 480.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    ctx.root_ui(|ui| {
                        assert!(app.render_grid(ui).is_none());
                    });
                },
            )
            .drop_without_applying_deltas();
        };
        draw(Vec::new());
        for pressed in [true, false, true, false] {
            draw(vec![egui::Event::PointerButton {
                pos: egui::pos2(250., 120.),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        draw(Vec::new());
        assert_eq!(notice_count(&app), 2, "{view:?}");
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(app.checked.is_empty());
    }
}

#[test]
fn page_edit_920_background_menu_and_preopened_menu_commands_are_rejected() {
    let mut app = linked_editing_app();
    let ctx = egui::Context::default();
    let owner = app.capture_grid_context_menu_owner(usize::MAX);
    assert!(!app.grid_context_menu_owner_is_current(owner));
    app.context_menu_idx = Some(owner);
    assert!(app.show_context_menu(&ctx).is_none());
    assert!(app.context_menu_idx.is_none());
    let root = app.tmp.path().to_owned();
    app.dispatch_grid_paste_command_for_test(&ctx, root);
    assert!(app.shell_paste_request.is_none());
    let item = app.items[0].clone();
    app.dispatch_grid_item_command_for_test(
        &ctx,
        item,
        crate::context_menu_model::MenuCommand::MoveToRecycleBin,
    );
    assert!(!app.show_delete_confirm);
    assert_eq!(
        notice_count(&app),
        1,
        "same-pass refusals replace one notice"
    );
    assert_eq!(
        app.fs_feedback_toast_surface,
        Some(ActionSurface::MainWindow)
    );
    app.conceal_mode = false;
    assert!(app.grid_context_menu_owner_is_current(owner));
}
