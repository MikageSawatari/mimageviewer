use super::*;
use crate::app::{AppTestEnvForTest, NormalFolderOmittedEntries, setup_app_for_test};
use crate::grid_item::GridItem;
use crate::main_drop_target::WindowMetrics;
use crate::shell_file_ops::{ShellTransferOperation, ShellTransferOutcome};
use crate::ui_dialogs::file_organize::FileOrganizeSelection;

#[test]
fn main_drop_tree_paste_menu_keyboard_owner_survives_early_close_and_resumes_next_pass() {
    let mut app = ready();
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    let root = app.tmp.path().to_owned();
    app.current_folder = Some(root.clone());
    app.settings.folder_tree_pane_visible = true;
    app.folder_pane = crate::folder_pane::FolderPaneState::test_tree(&root, vec![]);
    app.folder_pane.set_focus_grid();
    let popup = crate::ui_folder_pane::folder_pane_paste_menu_id();
    ctx.run_pass(Default::default(), |ctx| egui::Popup::open_id(ctx, popup))
        .drop_without_applying_deltas();
    ctx.run_pass(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            // Cover a menu closing before the App collects this pass's owner.
            egui::Popup::close_id(ctx, popup);
            app.input_update_begin(ctx);
            assert_eq!(
                app.keyboard_owner_for_pass(ctx),
                crate::keyboard_input::KeyboardOwner::Modal
            );
            assert!(app.handle_keyboard(ctx).is_none());
            app.input_main_grid_shortcuts(ctx, false);
            assert!(!app.folder_pane.has_focus);
            assert!(!app.show_open_folder_dialog);
        },
    )
    .drop_without_applying_deltas();
    ctx.run_pass(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::O,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL,
            }],
            ..Default::default()
        },
        |ctx| {
            app.input_update_begin(ctx);
            assert!(matches!(
                app.keyboard_owner_for_pass(ctx),
                crate::keyboard_input::KeyboardOwner::ApplicationShortcut { .. }
            ));
            assert!(app.handle_keyboard(ctx).is_none());
            app.input_main_grid_shortcuts(ctx, false);
            assert!(app.show_open_folder_dialog);
        },
    )
    .drop_without_applying_deltas();
}

#[test]
fn main_drop_unrelated_popup_keeps_text_input_owner_and_ime_dialog_keys() {
    use crate::keyboard_input::{KeyboardOwner, TextInputPhase};
    for composing in [false, true] {
        for early_close in [false, true] {
            let app = ready();
            let ctx = egui::Context::default();
            crate::ime_focus::install_ime_input_policy(&ctx);
            let field = egui::Id::unique("main_drop_text_editor");
            let popup = egui::Id::unique("main_drop_unrelated_completion_or_menu");
            let mut text = "draft".to_owned();
            let mut focus = true;
            ctx.run_ui(Default::default(), |ui| {
                crate::ime_focus::show_singleline(ui, &mut text, Some(&mut focus), |edit| {
                    edit.id(field)
                });
                egui::Popup::open_id(ui.ctx(), popup);
            })
            .drop_without_applying_deltas();
            let mut events = vec![
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            if composing {
                events.insert(
                    0,
                    egui::Event::Ime(egui::ImeEvent::Preedit {
                        text: "あ".into(),
                        active_range_chars: None,
                    }),
                );
            }
            ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                if early_close { egui::Popup::close_id(ui.ctx(), popup); }
                let owner = app.keyboard_owner_for_pass(ui.ctx());
                assert!(matches!(owner, KeyboardOwner::TextInput { widget_id, phase: TextInputPhase::Focused | TextInputPhase::FocusRecovery | TextInputPhase::ImeComposing, .. } if widget_id == field));
                assert!(!crate::ui_folder_pane::folder_pane_paste_menu_owns_keyboard(ui.ctx()));
                assert_eq!(app.ime_input_active(ui.ctx()), composing);
                assert_eq!(app.dialog_enter_pressed(ui.ctx()), !composing);
                // Existing generic popup Escape ownership is unchanged, even after early close.
                assert!(!app.dialog_escape_pressed(ui.ctx()));
                crate::ime_focus::show_singleline(ui, &mut text, Some(&mut focus), |edit| edit.id(field));
            }).drop_without_applying_deltas();
        }
    }
}

#[test]
fn main_drop_ctrl_f_keeps_existing_focused_input_rejection_and_unfocused_search_switch() {
    use crate::keyboard_input::{KeyboardOwner, TextInputPhase};
    for field in ["address", "local", "container", "metadata", "tag", "none"] {
        let mut app = ready();
        let ctx = egui::Context::default();
        app.settings.show_toolbar_search_local = false;
        app.show_search_bar = field == "local";
        app.favsearch.active = matches!(field, "address" | "container" | "none");
        app.global_search.active = field == "metadata";
        app.tag_view.active = field == "tag";
        app.address_has_focus = field == "address";
        app.search_has_focus = field == "local";
        app.favsearch.has_focus = field == "container";
        app.global_search.has_focus = field == "metadata";
        app.tag_view.has_focus = field == "tag";
        let before = (
            app.show_search_bar,
            app.favsearch.active,
            app.global_search.active,
            app.tag_view.active,
        );
        ctx.run_pass(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::F,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::CTRL,
                }],
                ..Default::default()
            },
            |ctx| {
                app.input_update_begin(ctx);
                if field != "none" {
                    assert!(matches!(
                        app.keyboard_owner_for_pass(ctx),
                        KeyboardOwner::TextInput {
                            phase: TextInputPhase::Focused,
                            ..
                        }
                    ));
                }
                app.input_main_grid_shortcuts(ctx, false);
                let after = (
                    app.show_search_bar,
                    app.favsearch.active,
                    app.global_search.active,
                    app.tag_view.active,
                );
                assert_eq!(
                    after,
                    if field == "none" {
                        (true, false, false, false)
                    } else {
                        before
                    },
                    "{field}"
                );
                if field != "none" {
                    assert!(
                        ctx.input(|input| input.key_pressed(egui::Key::F)),
                        "the editor retains Ctrl+F: {field}"
                    );
                }
            },
        )
        .drop_without_applying_deltas();
    }
}

fn ready() -> AppTestEnvForTest {
    let mut app = setup_app_for_test();
    app.indexer_init.mark_unavailable_if_not_ready();
    app.main_hwnd = Some(1);
    app.settings.book_root = Some(app.tmp.path().join("books"));
    app
}

fn grid_drop_app(view: crate::settings::GridViewMode) -> AppTestEnvForTest {
    let mut app = ready();
    let current = app.tmp.path().to_owned();
    app.current_folder = Some(current.clone());
    app.top_level_grid_view
        .replace_surface(super::super::top_level_grid_view::TopLevelGridSurface::Folder);
    app.normal_folder_omitted_entries = Some(NormalFolderOmittedEntries {
        folder: current.clone(),
        omitted: Default::default(),
    });
    app.items = vec![
        GridItem::Folder(current.join("folder-a")),
        GridItem::Image(current.join("image.jpg")),
        GridItem::Folder(current.join("folder-b")),
        GridItem::ZipFile(current.join("book.zip")),
        GridItem::Stack {
            key: "stack".into(),
            representative: current.join("stack.jpg"),
            count: 2,
        },
    ];
    app.thumbnails = vec![crate::grid_item::ThumbnailState::Failed; app.items.len()];
    app.image_metas = vec![None; app.items.len()];
    app.visible_indices = vec![2, 1, 0, 3, 4];
    app.details_order = app.visible_indices.clone();
    app.items_are_drive_list = false;
    app.settings.grid_view_mode = view;
    app.settings.grid_cols = 2;
    app.settings.details_selection_bar_mode = crate::settings::DetailsSelectionBarMode::Hidden;
    app.grid_scroll.set_reveal_requested(false);
    app
}

fn grid_drop_pass(app: &mut App, ctx: &egui::Context, overlay: Option<egui::Pos2>) -> DropGeometry {
    let mut geometry = None;
    ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 480.0),
            )),
            ..Default::default()
        },
        |ctx| {
            ctx.root_ui(|ui| {
                app.render_grid(ui);
            });
            if let Some(position) = overlay {
                paint_drop_test_overlay(ctx, "grid_drop_858_overlay", position);
            }
            geometry = Some(app.main_drop_geometry(
                ctx,
                WindowMetrics {
                    client_size_px: [640, 480],
                    dpi: 96,
                },
            ));
        },
    )
    .drop_without_applying_deltas();
    geometry.unwrap()
}

#[test]
fn grid_drop_858_real_app_pass_records_folder_paths_in_both_views() {
    use crate::settings::GridViewMode::{Details, Thumbnail};
    for view in [Thumbnail, Details] {
        let mut app = grid_drop_app(view);
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        grid_drop_pass(&mut app, &ctx, None);
        let geometry = grid_drop_pass(&mut app, &ctx, None);
        assert_eq!(geometry.grid_folders.len(), 2, "{view:?}");
        for row in &geometry.grid_folders {
            assert!(geometry.grid_clip.contains_rect(row.rect));
            assert!(
                app.items
                    .iter()
                    .any(|item| matches!(item, GridItem::Folder(path) if path == &row.path))
            );
        }
        assert_eq!(
            geometry.grid_folders[0].path,
            app.items[2].drag_source_path().unwrap()
        );
        assert!(geometry.grid_blocked.is_empty());
        ctx.run_pass(Default::default(), |ctx| {
            let stale = app.main_drop_geometry(
                ctx,
                WindowMetrics {
                    client_size_px: [640, 480],
                    dpi: 96,
                },
            );
            assert!(
                stale.grid_folders.is_empty(),
                "a pass without the grid cannot reuse it"
            );
            assert_eq!(stale.grid_clip, egui::Rect::NOTHING);
        })
        .drop_without_applying_deltas();
    }
}

#[test]
fn grid_drop_858_real_app_pass_nonqualifying_listings_have_no_grid_part() {
    use super::super::top_level_grid_view::{TopLevelGridSurface, TopLevelSearchView};
    for view in [
        crate::settings::GridViewMode::Thumbnail,
        crate::settings::GridViewMode::Details,
    ] {
        for listing in ["search", "missing-marker", "drive-list", "input-blocked"] {
            let mut app = grid_drop_app(view);
            let _held = (listing == "input-blocked").then(|| app.hold_book_structure_for_test());
            match listing {
                "search" => {
                    app.top_level_grid_view
                        .replace_surface(TopLevelGridSurface::Search(TopLevelSearchView::Global));
                }
                "missing-marker" => {
                    app.normal_folder_omitted_entries = None;
                }
                "drive-list" => {
                    app.top_level_grid_view
                        .replace_surface(TopLevelGridSurface::DriveList);
                    app.current_folder = None;
                    app.items_are_drive_list = true;
                    app.normal_folder_omitted_entries = None;
                }
                "input-blocked" => {
                    assert!(!app.grid_item_input_allowed());
                }
                _ => unreachable!(),
            }
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            let geometry = grid_drop_pass(&mut app, &ctx, None);
            assert!(geometry.grid_folders.is_empty(), "{view:?}/{listing}");
            assert!(geometry.grid_blocked.is_empty());
            assert_eq!(geometry.grid_clip, egui::Rect::NOTHING);
        }
    }
}

#[test]
fn grid_drop_858_real_app_pass_occluded_folder_moves_to_blocked() {
    for view in [
        crate::settings::GridViewMode::Thumbnail,
        crate::settings::GridViewMode::Details,
    ] {
        let mut app = grid_drop_app(view);
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let first = grid_drop_pass(&mut app, &ctx, None);
        let target = &first.grid_folders[0];
        let path = target.path.clone();
        let position = target.rect.min + egui::vec2(2.0, 2.0);
        grid_drop_pass(&mut app, &ctx, Some(position));
        let covered = grid_drop_pass(&mut app, &ctx, Some(position));
        assert!(!covered.grid_folders.iter().any(|row| row.path == path));
        assert!(
            covered
                .grid_blocked
                .iter()
                .any(|rect| rect.contains(position))
        );
    }
}

#[test]
fn grid_drop_858_details_horizontal_scroll_clips_before_tree_tooltip_occlusion() {
    let mut app = grid_drop_app(crate::settings::GridViewMode::Details);
    app.settings.details_name_width_auto = false;
    app.settings.details_name_width = 1200.0;
    app.selected = Some(2);
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let mut draw = |events: Vec<egui::Event>, tooltip: bool| {
        let mut result = None;
        ctx.run_pass(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 480.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|ui| {
                    egui::SidePanel::left("grid_drop_858_tree_space")
                        .exact_width(200.0)
                        .show(ui, |ui| {
                            ui.label("フォルダ");
                        });
                    app.render_grid(ui);
                });
                let captured = app.capture_main_drop_geometry_with_metrics(
                    ctx,
                    WindowMetrics {
                        client_size_px: [640, 480],
                        dpi: 96,
                    },
                );
                let row = captured.geometry.grid_folders[0].rect;
                let selection_fill = ctx.style().visuals.selection.bg_fill;
                let raw_row = ctx.graphics(|graphics| {
                    graphics
                        .get(egui::LayerId::background())
                        .unwrap()
                        .all_entries()
                        .find_map(|entry| match &entry.shape {
                            egui::Shape::Rect(shape)
                                if shape.fill == selection_fill
                                    && shape.rect.height() == 28.0
                                    && shape.rect.width() > 1000.0 =>
                            {
                                Some(shape.rect)
                            }
                            _ => None,
                        })
                        .unwrap()
                });
                if tooltip {
                    egui::Area::new(egui::Id::unique("grid_drop_858_tree_tooltip"))
                        .order(egui::Order::Tooltip)
                        .fixed_pos(egui::pos2(20.0, row.top() + 2.0))
                        .show(ctx, |ui| {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(30.0, 10.0), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
                        });
                }
                result = Some((finalize_main_drop_geometry(ctx, captured), raw_row));
            },
        )
        .drop_without_applying_deltas();
        result.unwrap()
    };
    let (first, raw_before) = draw(vec![], false);
    let point = first.grid_folders[0].rect.center();
    draw(
        vec![
            egui::Event::PointerMoved(point),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-320.0, 0.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
            },
        ],
        false,
    );
    for _ in 0..12 {
        draw(vec![], false);
    }
    draw(vec![], true);
    let (geometry, raw_after) = draw(vec![], true);
    assert!(
        raw_after.left() < raw_before.left() - 100.0,
        "fixture must really scroll horizontally"
    );
    assert!(
        raw_after.left() < 50.0,
        "the uncut row overlaps the tree tooltip"
    );
    assert_eq!(geometry.grid_folders.len(), 2);
    assert!(
        geometry.grid_blocked.is_empty(),
        "a tooltip outside the effective clip cannot block the row"
    );
    assert!(geometry.grid_clip.left() >= 200.0);
    for row in &geometry.grid_folders {
        assert_eq!(row.rect.left(), geometry.grid_clip.left());
        assert!(geometry.grid_clip.contains_rect(row.rect));
    }
}

#[test]
fn hover_860_app_matches_path_and_place_without_highlighting_other_items() {
    use crate::main_drop_target::{DropHover, DropPlace};
    let app = grid_drop_app(crate::settings::GridViewMode::Thumbnail);
    let target = app.items[2].drag_source_path().unwrap().to_owned();
    app.main_drop.borrow_mut().hover = Some(DropHover {
        path: target.clone(),
        place: DropPlace::Grid,
    });
    assert!(app.grid_drop_hover(2));
    for idx in [0, 1, 3, 4] {
        assert!(!app.grid_drop_hover(idx));
    }
    assert!(!app.main_drop_hover_matches(&target, DropPlace::Tree));
    app.main_drop.borrow_mut().hover = Some(DropHover {
        path: target.clone(),
        place: DropPlace::Tree,
    });
    assert!(!app.grid_drop_hover(2));
    assert!(app.main_drop_hover_matches(&target, DropPlace::Tree));
    app.main_drop.borrow_mut().hover = None;
    assert!(!app.grid_drop_hover(2));
}

#[test]
fn hover_860_details_folder_name_uses_selection_color() {
    use crate::main_drop_target::{DropHover, DropPlace};
    for theme in [
        crate::os_theme::ResolvedTheme::Light,
        crate::os_theme::ResolvedTheme::Dark,
    ] {
        let mut app = grid_drop_app(crate::settings::GridViewMode::Details);
        app.selected = None;
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        crate::os_theme::apply_resolved(&ctx, theme);
        let fill = ctx.style().visuals.selection.bg_fill;
        let rgb = [fill.r(), fill.g(), fill.b()];
        app.settings.details_name_colors.colors[0] =
            crate::details_name_colors::DetailsNameColor::Custom {
                light: rgb,
                dark: rgb,
            };
        app.main_drop.borrow_mut().hover = Some(DropHover {
            path: app.items[2].drag_source_path().unwrap().to_owned(),
            place: DropPlace::Grid,
        });
        grid_drop_pass(&mut app, &ctx, None);
        let expected = ctx.style().visuals.selection.stroke.color;
        ctx.run_pass(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 480.0),
                )),
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|ui| {
                    app.render_grid(ui);
                });
                let actual = ctx.graphics(|graphics| {
                    graphics
                        .get(egui::LayerId::background())
                        .unwrap()
                        .all_entries()
                        .find_map(|entry| match &entry.shape {
                            egui::Shape::Text(text) if text.galley.job.text == "folder-b" => {
                                Some(text.override_text_color.unwrap_or(text.fallback_color))
                            }
                            _ => None,
                        })
                });
                assert_eq!(
                    actual,
                    Some(expected),
                    "hovered folder name must match its selected background"
                );
            },
        )
        .drop_without_applying_deltas();
    }
}

fn transfer(destination: PathBuf) -> ShellTransferRequest {
    ShellTransferRequest {
        sources: vec![PathBuf::from("C:/source/a.jpg")],
        destination,
        operation: ShellTransferOperation::Copy,
        default_move_needs_volume_check: false,
    }
}

fn selecting(request: &ShellTransferRequest) -> FileOrganizeRequest {
    FileOrganizeRequest::Selecting(Box::new(FileOrganizeSelection {
        sources: request.sources.clone(),
        destinations: vec![crate::settings::FileOrganizeDestination {
            name: "destination".into(),
            path: request.destination.clone(),
        }],
        focus: None,
    }))
}

fn paint_drop_test_overlay(ctx: &egui::Context, id: &'static str, position: egui::Pos2) {
    egui::Area::new(egui::Id::unique(id))
        .order(egui::Order::Foreground)
        .fixed_pos(position)
        .show(ctx, |ui| {
            ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(ui.min_rect(), 0.0, egui::Color32::RED);
        });
}

#[test]
fn main_drop_admission_snapshot_and_both_submission_routes_reject_the_same_blockers() {
    // A Selecting owner rejects tree submission; organize consumes its own selection first.
    for blocker in [
        "shutdown",
        "root-close",
        "no-hwnd",
        "zero-hwnd",
        "modal",
        "viewer-modal",
        "sidecar",
        "remote",
        "snapshot",
    ] {
        for organize in [false, true] {
            let mut app = ready();
            let ctx = egui::Context::default();
            let mut input = egui::RawInput::default();
            if blocker == "root-close" {
                input
                    .viewports
                    .get_mut(&egui::ViewportId::ROOT)
                    .unwrap()
                    .events
                    .push(egui::ViewportEvent::Close);
            }
            ctx.begin_pass(input);
            let request = transfer(app.tmp.path().join("destination"));
            match blocker {
                "shutdown" => app.shutdown_requested.store(true, Ordering::SeqCst),
                "no-hwnd" => app.main_hwnd = None,
                "zero-hwnd" => app.main_hwnd = Some(0),
                "modal" => app.show_stats_dialog = true,
                "viewer-modal" => {
                    let _held = app.hold_book_structure_for_test();
                }
                "sidecar" => {
                    let folder = app.tmp.path().join("restore");
                    app.activate_sidecar_restore_modal_for_test(folder);
                }
                "remote" => {
                    let handle = crate::remote_ipc::session::SessionHandle::new();
                    handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
                        client_id: "tree-drop-test".into(),
                        peer: mimageviewer_ipc::SessionPeerInfo {
                            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                            device_name: Some("test".into()),
                        },
                    });
                    app.set_remote_session_handle(handle);
                }
                "snapshot" => {
                    let folder = app.tmp.path().to_owned();
                    let image = folder.join("image.jpg");
                    crate::app::seed_snapshot_context_for_test(
                        &mut app,
                        &folder.to_string_lossy(),
                        &[&image.to_string_lossy()],
                    );
                    app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
                    assert!(app.folder_pane_disabled());
                }
                _ => {}
            }
            let revision = app.book_append_undo_revision;
            let expected = app.shell_transfer_admission(&ctx);
            assert!(expected.is_err(), "{blocker}");
            app.refresh_main_drop_admission(&ctx);
            assert_eq!(app.main_drop.borrow().admission, expected, "{blocker}");
            assert_eq!(
                app.book_append_undo_revision, revision,
                "read-only admission must not retire undo"
            );
            if organize {
                app.file_organize_request = selecting(&request);
                app.submit_file_organize_with(
                    &ctx,
                    0,
                    request.operation,
                    |_| panic!("blocked request resolved a root"),
                    |_, _, _| panic!("organize spawned for {blocker}"),
                );
            } else {
                app.submit_shell_transfer_with(
                    &ctx,
                    request,
                    |_| panic!("blocked request resolved a root"),
                    |_, _, _| panic!("tree spawned for {blocker}"),
                );
            }
            assert!(
                matches!(app.file_organize_request, FileOrganizeRequest::Hidden),
                "{blocker}"
            );
            assert_eq!(
                app.book_append_undo_revision, revision,
                "rejection must not retire undo"
            );
            ctx.end_pass().drop_without_applying_deltas();
        }
    }
}

#[test]
fn main_drop_selecting_and_running_owners_survive_tree_or_duplicate_organize_submission() {
    for running in [false, true] {
        let mut app = ready();
        let ctx = egui::Context::default();
        let request = transfer(app.tmp.path().join("destination"));
        let (_held, rx) = mpsc::channel();
        app.file_organize_request = if running {
            FileOrganizeRequest::Running(Box::new(FileOrganizeRunning {
                request: request.clone(),
                rx,
            }))
        } else {
            selecting(&request)
        };
        let revision = app.book_append_undo_revision;
        assert!(app.shell_transfer_admission(&ctx).is_err());
        app.submit_shell_transfer_with(
            &ctx,
            request,
            |_| panic!("second request resolved a root"),
            |_, _, _| panic!("second tree worker"),
        );
        assert_eq!(app.book_append_undo_revision, revision);
        if running {
            app.submit_file_organize_with(
                &ctx,
                0,
                ShellTransferOperation::Copy,
                |_| panic!("duplicate request resolved a root"),
                |_, _, _| panic!("duplicate organize worker"),
            );
            assert!(matches!(
                app.file_organize_request,
                FileOrganizeRequest::Running(_)
            ));
        } else {
            assert!(matches!(
                app.file_organize_request,
                FileOrganizeRequest::Selecting(_)
            ));
        }
    }
}

#[test]
fn main_drop_both_routes_share_running_owner_and_one_mutation_admission() {
    for organize in [false, true] {
        let mut app = ready();
        let ctx = egui::Context::default();
        let request = transfer(app.tmp.path().join("destination"));
        let revision = app.book_append_undo_revision;
        let mut calls = 0;
        let (_held, rx) = mpsc::channel();
        let start = |hwnd, actual: ShellTransferRequest, _: egui::Context| {
            calls += 1;
            assert_eq!(hwnd, Some(1));
            assert_eq!(actual.sources, request.sources);
            assert_eq!(actual.destination, request.destination);
            assert_eq!(actual.operation, ShellTransferOperation::Copy);
            rx
        };
        if organize {
            app.file_organize_request = selecting(&request);
            app.submit_file_organize_with(
                &ctx,
                0,
                ShellTransferOperation::Copy,
                App::book_root_path,
                start,
            );
        } else {
            app.submit_shell_transfer_with(&ctx, request.clone(), App::book_root_path, start);
        }
        assert_eq!(calls, 1);
        assert!(matches!(
            app.file_organize_request,
            FileOrganizeRequest::Running(_)
        ));
        assert_eq!(app.book_append_undo_revision, revision.wrapping_add(1));
        assert!(app.main_drop.borrow().admission.is_err());
    }
}

#[test]
fn main_drop_grid_requests_are_drained_without_holding_the_shared_borrow() {
    let mut app = ready();
    let ctx = egui::Context::default();
    let paths = vec![
        PathBuf::from("C:/source/file"),
        PathBuf::from("C:/source/folder"),
    ];
    app.main_drop
        .borrow_mut()
        .drops
        .push(DropRequest::GridFiles(paths.clone()));
    let mut received = Vec::new();
    app.consume_main_drops_with(&ctx, |app, paths| {
        assert!(app.main_drop.try_borrow_mut().is_ok());
        received.push(paths);
    });
    assert_eq!(received, vec![paths.clone()]);
    assert!(app.main_drop.borrow().drops.is_empty());
    let restore = app.tmp.path().join("restore");
    app.activate_sidecar_restore_modal_for_test(restore);
    app.main_drop
        .borrow_mut()
        .drops
        .push(DropRequest::GridFiles(paths));
    app.consume_main_drops_with(&ctx, |_, _| panic!("sidecar drop reached the grid"));
    assert!(app.main_drop.borrow().drops.is_empty());
}

#[test]
fn main_drop_production_grid_routing_reaches_existing_search_refusal() {
    let mut app = ready();
    app.items_are_global_search_view = true;
    app.main_drop
        .borrow_mut()
        .drops
        .push(DropRequest::GridFiles(vec![PathBuf::from(
            "C:/source/a.jpg",
        )]));
    app.consume_main_drops(&egui::Context::default());
    assert!(
        app.fs_feedback_toast
            .as_ref()
            .unwrap()
            .0
            .contains("検索結果")
    );
    assert!(app.drop_copy_pending.is_empty());
}

#[test]
fn main_drop_858_completed_transfers_observe_each_parent_and_notify_current_folder_once() {
    for terminal in 0..8 {
        let mut app = ready();
        let ctx = egui::Context::default();
        let root = app.tmp.path().join("tree");
        let source = root.join("source");
        let other_source = root.join("other-source");
        let destination = root.join("destination");
        for folder in [&source, &other_source, &destination] {
            std::fs::create_dir_all(folder).unwrap();
        }
        app.folder_pane = crate::folder_pane::FolderPaneState::test_tree(
            &root,
            vec![source.clone(), other_source.clone(), destination.clone()],
        );
        for folder in [&source, &other_source, &destination] {
            app.folder_pane.test_seed_listing(folder, vec![], false);
            assert!(!app.folder_pane.test_unverified(folder));
        }
        app.current_folder = Some(source.clone());
        app.current_folder_last_mtime = Some(std::time::SystemTime::now());
        app.top_level_grid_view
            .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::Folder);
        app.normal_folder_omitted_entries = Some(NormalFolderOmittedEntries {
            folder: source.clone(),
            omitted: Default::default(),
        });
        let mut request = transfer(destination.clone());
        request.sources = vec![
            source.join("a.jpg"),
            source.join("b.jpg"),
            other_source.join("c.jpg"),
        ];
        let (tx, rx) = mpsc::channel();
        tx.send(match terminal {
            0 => Ok(ShellTransferOutcome::test_outcome(0, Ok(false), 0)),
            1 => Ok(ShellTransferOutcome::test_outcome(0, Ok(true), 0)),
            3 => Ok(ShellTransferOutcome::test_outcome(0, Ok(true), 1)),
            4 => Ok(ShellTransferOutcome::test_outcome(
                0x80070005u32 as i32,
                Ok(false),
                1,
            )),
            5 => Ok(ShellTransferOutcome::test_outcome(
                0,
                Err(0x80004005u32 as i32),
                0,
            )),
            6 | 7 => {
                let mut outcome = ShellTransferOutcome::test_outcome(
                    if terminal == 6 {
                        0
                    } else {
                        0x80070005u32 as i32
                    },
                    Ok(terminal == 6),
                    0,
                );
                outcome.counts.failed = 1;
                outcome.counts.first_failed_hresult = Some(0x80270027u32 as i32);
                Ok(outcome)
            }
            _ => Err("fake transfer failure".into()),
        })
        .unwrap();
        app.file_organize_request =
            FileOrganizeRequest::Running(Box::new(FileOrganizeRunning { request, rx }));
        app.poll_file_organize(&ctx);
        let expected = match terminal {
            0 => None,
            1 => Some("操作が中断されました。移動・コピーした項目はありません"),
            2 => Some(
                "操作を完了できませんでした。フォルダの内容を確認してください\nfake transfer failure",
            ),
            3 => Some(
                "操作が中断されました。一部の項目は処理済みです。フォルダの内容を確認してください",
            ),
            4 => Some(
                "操作を完了できませんでした。フォルダの内容を確認してください。一部の項目は処理済みです\nShell: 0x80070005、中断状態: 中断なし",
            ),
            5 => Some(
                "操作を完了できませんでした。フォルダの内容を確認してください\nShell: 0x00000000、中断状態: 照会失敗 0x80004005",
            ),
            6 => Some(
                "操作が中断されました。移動・コピーした項目はありません\n失敗した項目: 1 件 (Shell: 0x80270027)",
            ),
            7 => Some(
                "操作を完了できませんでした。フォルダの内容を確認してください\nShell: 0x80070005、中断状態: 中断なし、項目: 0x80270027",
            ),
            _ => unreachable!(),
        };
        assert_eq!(
            app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
            expected
        );
        for folder in [&source, &other_source, &destination] {
            assert!(app.folder_pane.test_unverified(folder));
        }
        let pending = app
            .external_rescan_pending
            .take()
            .expect("one current-folder Notified refresh");
        assert_eq!(pending.folder, source);
        assert!(!pending.restart_requested);
        pending.cancel.store(true, Ordering::Relaxed);
        app.poll_file_organize(&ctx);
        assert!(
            app.external_rescan_pending.is_none(),
            "completion must not notify twice"
        );
    }
}

#[test]
fn main_drop_858_external_copy_poll_preserves_shared_notices_and_reload() {
    for (perform, abort, processed, expected) in [
        (
            0,
            Ok(true),
            0,
            "操作が中断されました。移動・コピーした項目はありません",
        ),
        (
            0,
            Ok(true),
            1,
            "操作が中断されました。一部の項目は処理済みです。フォルダの内容を確認してください",
        ),
        (
            0x80070005u32 as i32,
            Ok(false),
            1,
            "操作を完了できませんでした。フォルダの内容を確認してください。一部の項目は処理済みです\nShell: 0x80070005、中断状態: 中断なし",
        ),
    ] {
        let mut app = ready();
        let copy = crate::file_drag::copy_dropped_paths_blocking(
            None,
            vec!["source.txt".into()],
            "destination".into(),
            |_, _| {
                Ok(ShellTransferOutcome::test_outcome(
                    perform, abort, processed,
                ))
            },
        );
        let (tx, rx) = mpsc::channel();
        tx.send(copy).unwrap();
        app.drop_copy_pending.push(rx);
        app.pending_reload = false;
        app.poll_file_drop_pending();
        assert!(app.pending_reload);
        assert!(app.drop_copy_pending.is_empty());
        assert_eq!(app.fs_feedback_toast.as_ref().unwrap().0, expected);
    }
}

#[test]
fn main_drop_paste_selection_belongs_only_to_the_current_real_folder() {
    for same_folder in [false, true] {
        for succeeds in [false, true] {
            let mut app = ready();
            let ctx = egui::Context::default();
            let current = app.tmp.path().join("current");
            let target = if same_folder {
                current.clone()
            } else {
                app.tmp.path().join("target")
            };
            app.current_folder = Some(current.clone());
            app.current_folder_last_mtime = Some(std::time::SystemTime::now());
            app.top_level_grid_view
                .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::Folder);
            app.normal_folder_omitted_entries = Some(NormalFolderOmittedEntries {
                folder: current.clone(),
                omitted: Default::default(),
            });
            app.request_post_operation_selection_for_added_items(current.clone());
            let revision = app.book_append_undo_revision;
            let mut calls = 0;
            app.paste_files_to_folder_with(&ctx, target.clone(), |hwnd, actual| {
                assert_eq!(hwnd, 1);
                assert_eq!(actual, target);
                calls += 1;
                if succeeds {
                    Ok(())
                } else {
                    Err("fake paste failure".into())
                }
            });
            assert_eq!(calls, 1);
            assert_eq!(app.book_append_undo_revision, revision.wrapping_add(1));
            assert_eq!(
                app.post_operation_selection.is_some(),
                succeeds || !same_folder
            );
            if !same_folder {
                let request = app.post_operation_selection.as_ref().unwrap();
                assert!(matches!(
                    crate::post_operation_selection::decide(
                        request,
                        Some(&current),
                        &[],
                        std::time::Instant::now()
                    ),
                    crate::post_operation_selection::Step::Wait
                ));
            }
        }
    }
}

#[test]
fn main_drop_paste_to_other_folder_creates_no_selection_and_ctrl_v_uses_the_same_helper() {
    let mut app = ready();
    let ctx = egui::Context::default();
    let current = app.tmp.path().join("current");
    app.current_folder = Some(current.clone());
    app.current_folder_last_mtime = Some(std::time::SystemTime::now());
    app.top_level_grid_view
        .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::Folder);
    app.normal_folder_omitted_entries = Some(NormalFolderOmittedEntries {
        folder: current.clone(),
        omitted: Default::default(),
    });
    let other = app.tmp.path().join("other");
    take_books_root_resolutions_for_test();
    app.paste_files_to_folder_with(&ctx, other, |_, _| Ok(()));
    assert!(app.post_operation_selection.is_none());
    assert_eq!(
        take_books_root_resolutions_for_test(),
        0,
        "the shared paste helper adds no root lookup"
    );
    let mut invocations = 0;
    app.handle_clipboard_paste_with(
        &ctx,
        || Ok(crate::clipboard_capture::data::ClipboardKind::Files),
        |app, ctx, _, _, folder| {
            app.paste_files_to_folder_with(ctx, folder, |_, actual| {
                assert_eq!(actual, current);
                invocations += 1;
                Ok(())
            });
        },
    );
    assert_eq!(invocations, 1);
    assert!(app.post_operation_selection.is_some());
    assert_eq!(
        take_books_root_resolutions_for_test(),
        1,
        "Ctrl+V keeps only current_favorite_target's existing lookup"
    );
}

#[test]
fn main_drop_geometry_uses_complete_clipped_rows_and_removes_intersecting_front_layers() {
    let mut app = ready();
    let ctx = egui::Context::default();
    let pane = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(240.0, 400.0));
    let clip = egui::Rect::from_min_max(egui::pos2(0.0, 40.0), egui::pos2(225.0, 380.0));
    let row = egui::Rect::from_min_max(egui::pos2(14.0, 50.0), egui::pos2(240.0, 76.0));
    let metrics = WindowMetrics {
        client_size_px: [800, 600],
        dpi: 144,
    };
    ctx.run_pass(Default::default(), |ctx| {
        paint_drop_test_overlay(ctx, "drop_front_test", egui::pos2(100.0, 60.0))
    })
    .drop_without_applying_deltas();
    for occlude in [false, true, false] {
        ctx.run_pass(Default::default(), |ctx| {
            if occlude {
                paint_drop_test_overlay(ctx, "drop_front_test", egui::pos2(100.0, 60.0));
            }
            app.folder_pane.label_display = Some(crate::folder_pane::FolderPaneLabelDisplay {
                pass: ctx.cumulative_pass_nr(),
                layer: egui::LayerId::background(),
                pane_rect: pane,
                rect: clip,
                rows: vec![crate::folder_pane::FolderPaneLabelWidgets {
                    path: PathBuf::from("C:/destination"),
                    row_rect: row,
                    label: egui::Id::NULL,
                    label_rect: row,
                    arrow: egui::Id::NULL,
                    arrow_rect: row,
                }],
            });
            let geometry = app.main_drop_geometry(ctx, metrics);
            assert_eq!(geometry.pane_rect, pane);
            assert_eq!(geometry.tree_clip, clip);
            assert_eq!(geometry.client_size_px, [800, 600]);
            assert_eq!(geometry.dpi, 144);
            if occlude {
                assert!(geometry.rows.is_empty());
            } else {
                assert_eq!(geometry.rows[0].rect, row.intersect(clip));
            }
        })
        .drop_without_applying_deltas();
    }
}

#[test]
fn main_drop_geometry_keeps_books_rows_and_clears_hidden_or_stale_panes() {
    let mut app = ready();
    let books_child = app.settings.book_root.as_ref().unwrap().join("Book");
    take_books_root_resolutions_for_test();
    let ctx = egui::Context::default();
    let metrics = WindowMetrics {
        client_size_px: [800, 600],
        dpi: 96,
    };
    ctx.run_pass(Default::default(), |ctx| {
        let pane = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 400.0));
        let row = egui::Rect::from_min_size(egui::pos2(14.0, 40.0), egui::vec2(180.0, 24.0));
        app.folder_pane.label_display = Some(crate::folder_pane::FolderPaneLabelDisplay {
            pass: ctx.cumulative_pass_nr(),
            layer: egui::LayerId::background(),
            pane_rect: pane,
            rect: pane,
            rows: vec![crate::folder_pane::FolderPaneLabelWidgets {
                path: books_child.clone(),
                row_rect: row,
                label: egui::Id::NULL,
                label_rect: row,
                arrow: egui::Id::NULL,
                arrow_rect: row,
            }],
        });
        let geometry = app.main_drop_geometry(ctx, metrics);
        assert_eq!(geometry.pane_rect, pane);
        assert!(
            geometry.rows.len() == 1,
            "books have ordinary geometry; the submission boundary refuses them"
        );
        app.folder_pane.label_display.as_mut().unwrap().pass = ctx.cumulative_pass_nr() + 1;
        assert_eq!(
            app.main_drop_geometry(ctx, metrics).pane_rect,
            egui::Rect::NOTHING
        );
        app.folder_pane.suspend_display();
        assert_eq!(
            app.main_drop_geometry(ctx, metrics).pane_rect,
            egui::Rect::NOTHING
        );
    })
    .drop_without_applying_deltas();
    assert_eq!(
        take_books_root_resolutions_for_test(),
        0,
        "geometry never resolves the bookshelf root"
    );
}

#[test]
fn main_drop_read_only_admission_keeps_an_existing_append_undo_receipt() {
    let mut app = ready();
    let root = app.tmp.path().join("undo-books");
    let folder = root.join("Book");
    std::fs::create_dir_all(&folder).unwrap();
    let mut builder =
        crate::book_append_undo::AppendReceiptBuilder::begin(&root, &folder, "Book".into())
            .unwrap();
    let output = folder.join("added.jpg");
    std::fs::write(&output, b"test-page").unwrap();
    builder.record(&output).unwrap();
    let receipt = crate::book_append_undo::BookAppendUndoReceipt {
        files: builder.finish().unwrap(),
        operation_id: 17,
        source_context: app.projected_viewer_context_id(),
        accepted_revision: app.book_append_undo_revision,
    };
    let operation = receipt.operation();
    app.book_append_undo_receipt = Some(receipt);
    let ctx = egui::Context::default();
    let revision = app.book_append_undo_revision;
    for blocked in [false, true] {
        app.show_stats_dialog = blocked;
        let _ = app.shell_transfer_admission(&ctx);
        app.refresh_main_drop_admission(&ctx);
        assert!(app.book_append_operation_matches_receipt(operation));
        assert_eq!(app.book_append_undo_revision, revision);
    }
}

#[test]
fn main_drop_native_read_failure_is_consumed_as_a_warning() {
    let mut app = ready();
    app.main_drop
        .borrow_mut()
        .drops
        .push(DropRequest::Failed("fake CF_HDROP failure".into()));
    app.consume_main_drops(&egui::Context::default());
    assert!(app.main_drop.borrow().drops.is_empty());
    assert!(
        app.fs_feedback_toast
            .as_ref()
            .unwrap()
            .0
            .contains("fake CF_HDROP failure")
    );
}

#[test]
fn main_drop_drawn_geometry_survives_input_retirement_and_uses_late_overlay_occlusion() {
    let mut app = ready();
    let ctx = egui::Context::default();
    let metrics = WindowMetrics {
        client_size_px: [800, 600],
        dpi: 96,
    };
    ctx.run_pass(Default::default(), |ctx| {
        paint_drop_test_overlay(ctx, "late_drop_overlay", egui::pos2(100.0, 48.0))
    })
    .drop_without_applying_deltas();
    ctx.run_pass(Default::default(), |ctx| {
        let pane = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(240.0, 400.0));
        let row = egui::Rect::from_min_size(egui::pos2(14.0, 40.0), egui::vec2(220.0, 24.0));
        app.folder_pane.label_display = Some(crate::folder_pane::FolderPaneLabelDisplay {
            pass: ctx.cumulative_pass_nr(),
            layer: egui::LayerId::background(),
            pane_rect: pane,
            rect: pane,
            rows: vec![crate::folder_pane::FolderPaneLabelWidgets {
                path: PathBuf::from("C:/destination"),
                row_rect: row,
                label: egui::Id::NULL,
                label_rect: row,
                arrow: egui::Id::NULL,
                arrow_rect: row,
            }],
        });
        let drawn = app.capture_main_drop_geometry_with_metrics(ctx, metrics);
        let covered = app.capture_main_drop_geometry_with_metrics(ctx, metrics);
        app.folder_pane.suspend_display();
        assert!(app.folder_pane.label_display.is_none());
        let geometry = finalize_main_drop_geometry(ctx, drawn);
        assert_eq!(
            geometry.pane_rect, pane,
            "a drawn pane remains pane-owned until another pass is presented"
        );
        assert_eq!(geometry.rows.len(), 1);
        paint_drop_test_overlay(ctx, "late_drop_overlay", egui::pos2(100.0, 48.0));
        let geometry = finalize_main_drop_geometry(ctx, covered);
        assert_eq!(geometry.pane_rect, pane);
        assert!(
            geometry.rows.is_empty(),
            "late dialogs occlude the already captured pane geometry"
        );
    })
    .drop_without_applying_deltas();
}

#[test]
fn main_drop_pass_publication_never_resolves_an_unconfigured_books_root() {
    let mut app = ready();
    app.settings.book_root = None;
    let ctx = egui::Context::default();
    let metrics = WindowMetrics {
        client_size_px: [800, 600],
        dpi: 96,
    };
    take_books_root_resolutions_for_test();
    for _ in 0..3 {
        ctx.run_pass(Default::default(), |ctx| {
            let pane = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(240.0, 400.0));
            app.folder_pane.label_display = Some(crate::folder_pane::FolderPaneLabelDisplay {
                pass: ctx.cumulative_pass_nr(),
                layer: egui::LayerId::background(),
                pane_rect: pane,
                rect: pane,
                rows: vec![crate::folder_pane::FolderPaneLabelWidgets {
                    path: PathBuf::from("C:/books/Book"),
                    row_rect: pane,
                    label: egui::Id::NULL,
                    label_rect: pane,
                    arrow: egui::Id::NULL,
                    arrow_rect: pane,
                }],
            });
            assert!(app.shell_transfer_admission(ctx).is_ok());
            app.refresh_main_drop_admission(ctx);
            let captured = app.capture_main_drop_geometry_with_metrics(ctx, metrics);
            assert_eq!(captured.geometry.rows.len(), 1);
            app.publish_main_drop_geometry(ctx, Some(captured));
            assert!(app.main_drop.borrow().admission.is_ok());
        })
        .drop_without_applying_deltas();
    }
    assert_eq!(take_books_root_resolutions_for_test(), 0);
}

#[test]
fn main_drop_books_rejection_and_default_resolution_are_shared_by_all_operation_boundaries() {
    use crate::post_operation_selection::{ExpectedOutputs, Step};
    for route in ["tree", "organize", "paste"] {
        for location in ["root", "child", "outside"] {
            let mut app = ready();
            let ctx = egui::Context::default();
            let books_root = app.tmp.path().join("resolved-default-books");
            let target = match location {
                "root" => books_root.clone(),
                "child" => books_root.join("Book/subfolder"),
                _ => app.tmp.path().join("outside"),
            };
            app.settings.book_root = None;
            app.current_folder = Some(target.clone());
            app.current_folder_last_mtime = Some(std::time::SystemTime::now());
            app.top_level_grid_view
                .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::Folder);
            app.normal_folder_omitted_entries = Some(NormalFolderOmittedEntries {
                folder: target.clone(),
                omitted: Default::default(),
            });
            let prior_output = target.join("prior.jpg");
            app.items = vec![GridItem::Image(prior_output.clone())];
            app.selected = Some(0);
            app.checked.insert(0);
            app.request_post_operation_selection(
                target.clone(),
                ExpectedOutputs::Known(vec![prior_output.clone()]),
            );
            let revision = app.book_append_undo_revision;
            let mut resolutions = 0;
            let mut executions = 0;
            let resolver = |app: &App| {
                assert!(app.settings.book_root.is_none());
                resolutions += 1;
                books_root.clone()
            };
            take_books_root_resolutions_for_test();
            if route == "paste" {
                app.paste_tree_files_to_folder_with(
                    &ctx,
                    target.clone(),
                    resolver,
                    |hwnd, actual| {
                        assert_eq!(hwnd, 1);
                        assert_eq!(actual, target);
                        executions += 1;
                        Ok(())
                    },
                );
            } else {
                let request = transfer(target.clone());
                let (_held, rx) = mpsc::channel();
                let start = |hwnd, actual: ShellTransferRequest, _: egui::Context| {
                    assert_eq!(hwnd, Some(1));
                    assert_eq!(actual.destination, target);
                    executions += 1;
                    rx
                };
                if route == "organize" {
                    app.file_organize_request = selecting(&request);
                    app.submit_file_organize_with(&ctx, 0, request.operation, resolver, start);
                } else {
                    app.submit_shell_transfer_with(&ctx, request, resolver, start);
                }
            }
            assert_eq!(
                resolutions, 1,
                "{route}/{location}: resolve once at the operation boundary"
            );
            assert_eq!(
                take_books_root_resolutions_for_test(),
                0,
                "the fake resolver must avoid the OS getter"
            );
            if location == "outside" {
                assert_eq!(executions, 1);
                assert_eq!(app.book_append_undo_revision, revision.wrapping_add(1));
            } else {
                assert_eq!(executions, 0, "{route}/{location}");
                assert_eq!(
                    app.book_append_undo_revision, revision,
                    "books refusal must precede mutation admission"
                );
                assert_eq!(app.selected, Some(0));
                assert!(app.checked.contains(&0));
                assert!(matches!(
                    app.file_organize_request,
                    FileOrganizeRequest::Hidden
                ));
                assert_eq!(
                    app.fs_feedback_toast.as_ref().unwrap().0,
                    "本棚の中へは移動・コピーできません"
                );
                assert_eq!(
                    crate::post_operation_selection::decide(
                        app.post_operation_selection.as_ref().unwrap(),
                        Some(&target),
                        &[(19, prior_output)],
                        std::time::Instant::now(),
                    ),
                    Step::Apply(vec![19]),
                    "rejection must retain the exact prior selection request"
                );
            }
        }
    }
}
