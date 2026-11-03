use super::section375_tests::{fixture, settle};
use super::*;
use crate::folder_pane::{FolderPaneCursorScroll, FolderPaneState};
use std::time::SystemTime;

fn tree_frame(app: &mut App, ctx: &egui::Context, frame: &mut u64, toggle: bool) {
    let events = if toggle {
        vec![egui::Event::Key {
            // The current default binding is F (not T).
            key: egui::Key::F,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    } else {
        vec![]
    };
    tree_event_frame(app, ctx, frame, events);
}

fn tree_event_frame(app: &mut App, ctx: &egui::Context, frame: &mut u64, events: Vec<egui::Event>) {
    *frame += 1;
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 360.0),
            )),
            time: Some(*frame as f64 / 60.0),
            events,
            ..Default::default()
        },
        |ui| {
            app.handle_keyboard(ctx);
            app.render_folder_pane(ui);
        },
    )
    .drop_without_applying_deltas();
}

fn startup_tree_grid_frame(
    app: &mut App,
    ctx: &egui::Context,
    frame: &mut u64,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) {
    *frame += 1;
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some(*frame as f64 / 60.0),
            events,
            ..Default::default()
        },
        |ui| {
            app.handle_keyboard(ctx);
            assert!(app.render_toolbar(ui).0.is_none());
            app.render_folder_pane(ui);
            assert!(app.render_grid(ui).is_none());
            app.show_first_setup_dialog(ctx);
        },
    )
    .drop_without_applying_deltas();
}

fn startup_tree_focus_scenario(startup_argument: bool, input_order: &str) {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    if input_order == "remapped" {
        app.keymap = crate::keymap::Keymap::from_ini_str("[Grid]\nGridToggleFolderTreePane=J\n");
    }
    let toggle_key = if input_order == "remapped" {
        egui::Key::J
    } else {
        egui::Key::F
    };
    app.settings.first_setup_completed = false;
    app.settings.ai_feature_mode = crate::settings::AiFeatureMode::Disabled;
    app.settings.auto_fullscreen_zip_pdf = false;
    app.settings.grid_click_selection_mode = crate::settings::GridClickSelectionMode::Explorer;
    app.settings.folder_tree_pane_visible = false;
    let root = app.tmp.path().join("startup-tree");
    let mut target = root.clone();
    for depth in 1..=4 {
        std::fs::create_dir_all(&target).unwrap();
        for index in 0..40 {
            std::fs::create_dir(target.join(format!("L{depth}-sib{index:02}"))).unwrap();
        }
        target = target.join(format!("L{depth}-zz-path"));
    }
    target = target.join("target");
    std::fs::create_dir_all(&target).unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(8, 8)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    for index in 0..3 {
        std::fs::write(target.join(format!("img{index}.png")), png.get_ref()).unwrap();
    }
    app.folder_pane.test_scan_subtree(root);
    if startup_argument {
        app.start_startup_open_path_resolve(
            target.clone(),
            StartupOpenPathSource::InitialStartup,
            &ctx,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.startup_open_path_resolve_pending.is_some() {
            app.poll_startup_open_path_resolve(&ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        settle(&mut app, &ctx);
    }
    let normal = egui::vec2(1280.0, 800.0);
    let maximized = egui::vec2(1920.0, 1080.0);
    let mut frame = 0;
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    startup_tree_grid_frame(&mut app, &ctx, &mut frame, normal, vec![]);
    assert!(app.first_setup_visible());
    startup_tree_grid_frame(
        &mut app,
        &ctx,
        &mut frame,
        normal,
        vec![key(egui::Key::Enter)],
    );
    assert!(
        !app.first_setup_visible(),
        "actual setup confirmation must complete"
    );
    if !startup_argument {
        app.load_folder(target.clone());
        settle(&mut app, &ctx);
    }
    assert_eq!(app.effective_folder(), Some(target.clone()));
    assert_eq!(app.items.len(), 3);
    assert!(!app.folder_pane_disabled());
    for _ in 0..3 {
        startup_tree_grid_frame(&mut app, &ctx, &mut frame, maximized, vec![]);
    }
    let pos = egui::pos2(1500.0, 900.0);
    let pointer = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    startup_tree_grid_frame(
        &mut app,
        &ctx,
        &mut frame,
        maximized,
        vec![egui::Event::PointerMoved(pos)],
    );
    if input_order != "whole_click" {
        startup_tree_grid_frame(&mut app, &ctx, &mut frame, maximized, vec![pointer(true)]);
    }
    let release = match input_order {
        "separate" | "toolbar" | "toolbar_coalesced" => vec![pointer(false)],
        "key_first" => vec![key(toggle_key), pointer(false)],
        "whole_click" => vec![pointer(true), pointer(false), key(toggle_key)],
        _ => vec![pointer(false), key(toggle_key)],
    };
    if input_order == "toolbar" || input_order == "toolbar_coalesced" {
        let rect: egui::Rect = ctx
            .data(|data| data.get_temp(egui::Id::unique("folder_pane_test_toolbar_rect")))
            .expect("actual toolbar button");
        let pos = rect.center();
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        if input_order == "toolbar" {
            startup_tree_grid_frame(&mut app, &ctx, &mut frame, maximized, release);
            startup_tree_grid_frame(
                &mut app,
                &ctx,
                &mut frame,
                maximized,
                vec![egui::Event::PointerMoved(pos), button(true)],
            );
            startup_tree_grid_frame(&mut app, &ctx, &mut frame, maximized, vec![button(false)]);
        } else {
            startup_tree_grid_frame(
                &mut app,
                &ctx,
                &mut frame,
                maximized,
                vec![
                    pointer(false),
                    egui::Event::PointerMoved(pos),
                    button(true),
                    button(false),
                ],
            );
        }
    } else {
        startup_tree_grid_frame(&mut app, &ctx, &mut frame, maximized, release);
    }
    if input_order == "separate" {
        startup_tree_grid_frame(&mut app, &ctx, &mut frame, maximized, vec![key(toggle_key)]);
    }
    assert!(app.settings.folder_tree_pane_visible);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.folder_pane.has_pending() {
        startup_tree_grid_frame(&mut app, &ctx, &mut frame, maximized, vec![]);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    for _ in 0..12 {
        startup_tree_grid_frame(&mut app, &ctx, &mut frame, maximized, vec![]);
    }
    assert_eq!(
        app.folder_pane.has_focus(),
        input_order != "key_first",
        "startup={startup_argument}, input order={input_order}"
    );
    assert_tree_location_row(&app, &ctx, &target);
}

#[test]
fn folder_pane_startup_argument_setup_click_then_f_keeps_focus() {
    startup_tree_focus_scenario(true, "separate");
}

#[test]
fn folder_pane_startup_argument_setup_grid_release_then_f_in_one_frame_keeps_focus() {
    startup_tree_focus_scenario(true, "release_first");
}

#[test]
fn folder_pane_address_open_setup_click_then_f_keeps_focus() {
    startup_tree_focus_scenario(false, "separate");
}

#[test]
fn folder_pane_startup_argument_setup_f_then_grid_release_keeps_grid_focus() {
    startup_tree_focus_scenario(true, "key_first");
}

#[test]
fn folder_pane_startup_argument_setup_whole_click_then_f_keeps_tree_focus() {
    startup_tree_focus_scenario(true, "whole_click");
}

#[test]
fn folder_pane_startup_argument_setup_release_then_remapped_key_keeps_tree_focus() {
    startup_tree_focus_scenario(true, "remapped");
}

#[test]
fn folder_pane_startup_argument_setup_toolbar_keeps_tree_focus() {
    startup_tree_focus_scenario(true, "toolbar");
}

#[test]
fn folder_pane_startup_argument_setup_grid_release_then_toolbar_in_one_frame_keeps_tree_focus() {
    startup_tree_focus_scenario(true, "toolbar_coalesced");
}

#[test]
fn folder_pane_later_escape_keeps_focus_and_cursor_without_dropping_earlier_row_actions() {
    for arrow in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let root = app.tmp.path().join("ordered-row");
        let a = root.join("a");
        let b = root.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(b.join("child")).unwrap();
        app.current_folder = Some(a.clone());
        app.items_are_drive_list = false;
        app.folder_pane.test_scan_subtree(root);
        app.set_folder_tree_pane_visible(true);
        let mut frame = 0;
        assert_tree_location(&mut app, &ctx, &mut frame, &a);
        app.folder_pane.set_focus_grid();
        let tag = if arrow {
            "folder_pane_test_arrow_id"
        } else {
            "folder_pane_test_label_id"
        };
        let id: egui::Id = ctx
            .data(|data| data.get_temp(egui::Id::unique((tag, b.as_path()))))
            .unwrap();
        let mut nav = None;
        frame += 1;
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 360.0),
                )),
                time: Some(frame as f64 / 60.0),
                events: vec![
                    egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
                        target_tree: egui::accesskit::TreeId::ROOT,
                        target_node: id.accesskit_id(),
                        action: egui::accesskit::Action::Click,
                        data: None,
                    }),
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            |ui| {
                app.handle_keyboard(&ctx);
                nav = app.render_folder_pane(ui).0;
            },
        )
        .drop_without_applying_deltas();
        assert!(app.folder_pane.has_focus());
        assert!(
            app.folder_pane
                .visible_rows()
                .iter()
                .any(|row| row.path == a && row.is_cursor)
        );
        if arrow {
            assert!(
                app.folder_pane
                    .visible_rows()
                    .iter()
                    .any(|row| row.path == b && row.expanded)
            );
            assert!(nav.is_none());
        } else {
            assert_eq!(
                nav,
                Some(b),
                "the older accepted Open is independent of focus arbitration"
            );
        }
    }
}

#[test]
fn folder_pane_ancestor_collapse_and_escape_respect_input_order_and_own_row_collapse() {
    for collapse_first in [true, false] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let root = app.tmp.path().join("ordered-collapse");
        let ancestor = root.join("branch");
        let active = ancestor.join("current");
        std::fs::create_dir_all(&active).unwrap();
        app.current_folder = Some(active.clone());
        app.items_are_drive_list = false;
        app.folder_pane.test_scan_subtree(root);
        app.set_folder_tree_pane_visible(true);
        let mut frame = 0;
        assert_tree_location(&mut app, &ctx, &mut frame, &active);
        app.folder_pane.set_focus_grid();
        let id: egui::Id = ctx
            .data(|data| {
                data.get_temp(egui::Id::unique((
                    "folder_pane_test_arrow_id",
                    ancestor.as_path(),
                )))
            })
            .unwrap();
        let collapse = egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: id.accesskit_id(),
            action: egui::accesskit::Action::Click,
            data: None,
        });
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        tree_event_frame(
            &mut app,
            &ctx,
            &mut frame,
            if collapse_first {
                vec![collapse, escape]
            } else {
                vec![escape, collapse]
            },
        );
        settle_tree_scans(&mut app, &ctx, &mut frame);
        let rows = app.folder_pane.visible_rows();
        let row = rows.iter().find(|row| row.path == ancestor).unwrap();
        assert!(app.folder_pane.has_focus());
        assert_eq!(row.expanded, collapse_first);
        assert_eq!(row.is_cursor, !collapse_first);
        if collapse_first {
            assert_tree_location(&mut app, &ctx, &mut frame, &active);
        } else {
            assert!(!rows.iter().any(|row| row.path == active));
            assert_tree_cursor_visible(&app, &ctx, &ancestor);
        }
    }
}

fn settle_tree_scans(app: &mut App, ctx: &egui::Context, frame: &mut u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        tree_frame(app, ctx, frame, false);
        if !app.folder_pane.has_pending() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "tree scans did not finish"
        );
        std::thread::yield_now();
    }
}

fn tree_scrolls(ctx: &egui::Context) -> usize {
    ctx.data(|data| data.get_temp(egui::Id::unique("folder_pane_test_cursor_scrolls")))
        .unwrap_or_default()
}

fn assert_tree_cursor_visible(app: &App, ctx: &egui::Context, expected: &Path) {
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == expected && row.is_cursor),
        "expected cursor: {expected:?}"
    );
    let rect = ctx
        .data(|data| {
            data.get_temp::<egui::Rect>(egui::Id::unique(("folder_pane_test_row_rect", expected)))
        })
        .unwrap();
    let viewport = ctx
        .data(|data| data.get_temp::<egui::Rect>(egui::Id::unique("folder_pane_test_viewport")))
        .unwrap();
    assert!(
        rect.top() >= viewport.top() && rect.bottom() <= viewport.bottom(),
        "{rect:?} / {viewport:?}"
    );
}

fn folder_pane_late_location(initial: &str, from_key: bool, other_drive: bool) {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let root = app.tmp.path().join("late-tree");
    let active = root.join("z-branch").join("z-current");
    std::fs::create_dir_all(&active).unwrap();
    for i in 0..80 {
        std::fs::create_dir(root.join(format!("a-{i:03}"))).unwrap();
    }
    app.folder_pane.test_scan_subtree(root.clone());
    let actual_drive = active.ancestors().last().unwrap().to_path_buf();
    let fallback = if other_drive {
        if actual_drive == Path::new(r"C:\") {
            PathBuf::from(r"G:\")
        } else {
            PathBuf::from(r"C:\")
        }
    } else {
        actual_drive
    };
    app.folder_pane.test_set_default_drive(Some(fallback));
    app.items_are_drive_list = false;
    app.current_folder = match initial {
        "zip" => Some(root.join("book.zip")),
        "pdf" => Some(root.join("book.pdf")),
        "search" => Some(search_results_synthetic_path()),
        _ => None,
    };
    app.settings.folder_tree_pane_visible = false;
    let mut frame = 0;
    if from_key {
        tree_frame(&mut app, &ctx, &mut frame, true);
    } else {
        app.set_folder_tree_pane_visible(true);
    }
    for _ in 0..4 {
        tree_frame(&mut app, &ctx, &mut frame, false);
    }
    assert!(app.settings.folder_tree_pane_visible && app.folder_pane.has_focus());
    app.current_folder = Some(active.clone());
    assert_tree_location(&mut app, &ctx, &mut frame, &active);
}

#[test]
fn folder_pane_late_active_menu_same_drive_is_revealed() {
    folder_pane_late_location("none", false, false);
}
#[test]
fn folder_pane_late_active_key_other_drive_is_revealed() {
    folder_pane_late_location("none", true, true);
}
#[test]
fn folder_pane_late_active_menu_other_drive_is_revealed() {
    folder_pane_late_location("none", false, true);
}
#[test]
fn folder_pane_late_active_key_same_drive_is_revealed() {
    folder_pane_late_location("none", true, false);
}
#[test]
fn folder_pane_search_to_physical_tracks_location() {
    folder_pane_late_location("search", false, false);
}
#[test]
fn folder_pane_zip_to_physical_tracks_location() {
    folder_pane_late_location("zip", false, false);
}
#[test]
fn folder_pane_pdf_to_physical_tracks_location() {
    folder_pane_late_location("pdf", true, false);
}

#[test]
fn folder_pane_explicit_arrow_then_active_change_and_grid_focus_keeps_f_commit() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let root = app.tmp.path().join("manual-tree");
    let a = root.join("a");
    let b = root.join("b");
    let c = root.join("c");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    std::fs::create_dir(&c).unwrap();
    app.folder_pane.test_scan_subtree(root.clone());
    app.current_folder = Some(a.clone());
    app.items_are_drive_list = false;
    app.set_folder_tree_pane_visible(true);
    let mut frame = 0;
    assert_tree_location(&mut app, &ctx, &mut frame, &a);
    tree_event_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::Key {
            key: egui::Key::ArrowDown,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    // Keep the same ancestor expanded, so this checks selection ownership
    // independently of the existing automatic-branch replacement behavior.
    app.current_folder = Some(c);
    settle_tree_scans(&mut app, &ctx, &mut frame);
    assert_tree_cursor_visible(&app, &ctx, &b);
    app.folder_pane.set_focus_grid();
    tree_frame(&mut app, &ctx, &mut frame, false);
    assert_tree_cursor_visible(&app, &ctx, &b);
    tree_frame(&mut app, &ctx, &mut frame, true);
    assert!(!app.settings.folder_tree_pane_visible);
    assert_eq!(
        app.folder_pane_open_pending
            .as_ref()
            .map(|pending| &pending.path),
        Some(&b),
        "F must still commit the explicit cursor after grid focus"
    );
}

fn pane_scroll_state(ctx: &egui::Context) -> egui::scroll_area::State {
    ctx.data(|data| data.get_temp(egui::Id::unique("folder_pane_test_scroll_state")))
        .unwrap()
}

#[test]
fn folder_pane_drag_inertia_then_arrow_reveal_stays_visible() {
    for (key, index, pending) in [
        (egui::Key::ArrowUp, 149, false),
        (egui::Key::ArrowDown, 151, true),
    ] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let root = app.tmp.path().join("inertia-tree");
        std::fs::create_dir(&root).unwrap();
        for i in 0..300 {
            std::fs::create_dir(root.join(format!("row-{i:03}"))).unwrap();
        }
        let active = root.join("row-150");
        app.current_folder = Some(active.clone());
        app.items_are_drive_list = false;
        app.folder_pane.test_scan_subtree(root.clone());
        app.set_folder_tree_pane_visible(true);
        let mut frame = 0;
        assert_tree_location(&mut app, &ctx, &mut frame, &active);
        let release = pending.then(|| {
            let release = app.folder_pane.test_hold_next_scan(root.clone());
            app.folder_pane.observe_accepted_folder(&root);
            release
        });
        let pointer = |y, pressed| egui::Event::PointerButton {
            pos: egui::pos2(80.0, y),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        tree_event_frame(
            &mut app,
            &ctx,
            &mut frame,
            vec![egui::Event::PointerMoved(egui::pos2(80.0, 260.0))],
        );
        tree_event_frame(&mut app, &ctx, &mut frame, vec![pointer(260.0, true)]);
        for y in [200.0, 140.0, 80.0] {
            tree_event_frame(
                &mut app,
                &ctx,
                &mut frame,
                vec![egui::Event::PointerMoved(egui::pos2(80.0, y))],
            );
        }
        tree_event_frame(&mut app, &ctx, &mut frame, vec![pointer(80.0, false)]);
        let moving = pane_scroll_state(&ctx);
        assert!(
            moving.velocity().y > 100.0,
            "real drag release must leave inertia: {:?}",
            moving.velocity()
        );
        tree_frame(&mut app, &ctx, &mut frame, false);
        assert!(
            pane_scroll_state(&ctx).unclamped_offset().y > moving.unclamped_offset().y,
            "normal inertia must advance"
        );
        tree_event_frame(
            &mut app,
            &ctx,
            &mut frame,
            vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(
            pane_scroll_state(&ctx).velocity().y,
            0.0,
            "new reveal must stop old inertia"
        );
        let cursor = root.join(format!("row-{index:03}"));
        for _ in 0..30 {
            tree_frame(&mut app, &ctx, &mut frame, false);
            assert_tree_cursor_visible(&app, &ctx, &cursor);
        }
        if let Some(release) = release {
            release.send(()).unwrap();
            settle_tree_scans(&mut app, &ctx, &mut frame);
        }
    }
}

fn assert_tree_location(app: &mut App, ctx: &egui::Context, frame: &mut u64, active: &Path) {
    settle_tree_scans(app, ctx, frame);
    let settled_scrolls = tree_scrolls(ctx);
    // Allow egui's scroll animation to finish, without changing the requested scroll.
    for _ in 0..90 {
        tree_frame(app, ctx, frame, false);
    }
    assert_eq!(
        tree_scrolls(ctx),
        settled_scrolls,
        "idle frames must not force cursor scrolling"
    );
    assert_eq!(app.folder_pane.cursor_scroll, FolderPaneCursorScroll::Idle);
    assert_tree_location_row(app, ctx, active);
}

fn assert_tree_location_row(app: &App, ctx: &egui::Context, active: &Path) {
    let rows = app.folder_pane.visible_rows();
    let row = rows
        .iter()
        .find(|row| row.path == active)
        .expect("(a) current location row");
    assert!(row.is_active, "(c) active location highlight");
    assert!(row.is_cursor, "(c) current location cursor");
    let rect = ctx
        .data(|data| {
            data.get_temp::<egui::Rect>(egui::Id::unique(("folder_pane_test_row_rect", active)))
        })
        .unwrap();
    let viewport = ctx
        .data(|data| data.get_temp::<egui::Rect>(egui::Id::unique("folder_pane_test_viewport")))
        .unwrap();
    assert!(
        rect.top() >= viewport.top() && rect.bottom() <= viewport.bottom(),
        "(b) current location must fit in viewport: row={rect:?}, viewport={viewport:?}"
    );
}

fn tree_open_scenario(route: &str) {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    // Real subtree scans, many preceding siblings, and multiple ancestors.
    // Ancestors above the fixture use synthetic listings, so the host drive's
    // protected/hidden directories cannot change the test result.
    let root = app.tmp.path().join("tree-open");
    let branch = root.join("z-branch");
    let active = branch.join("z-current");
    std::fs::create_dir_all(&active).unwrap();
    for parent in [&root, &branch] {
        for index in 0..80 {
            std::fs::create_dir(parent.join(format!("a-{index:03}"))).unwrap();
        }
    }
    app.folder_pane.test_scan_subtree(root.clone());
    app.current_folder = Some(active.clone());
    app.items_are_drive_list = false;
    app.settings.folder_tree_pane_visible = false;
    let mut frame = 0;
    if route == "startup_visible" {
        // App construction has no separate pane-visible startup path: the
        // loaded setting is read at the first chrome draw. Start visible there.
        app.settings.folder_tree_pane_visible = true;
    } else {
        tree_frame(&mut app, &ctx, &mut frame, false);
        assert!(
            app.folder_pane.active_path().is_none(),
            "hidden pane stays unsynchronized"
        );
        if route == "key" {
            tree_frame(&mut app, &ctx, &mut frame, true);
        } else {
            app.set_folder_tree_pane_visible(true);
        }
    }
    assert!(
        app.settings.folder_tree_pane_visible,
        "real opening entry ran"
    );
    assert_tree_location(&mut app, &ctx, &mut frame, &active);
    if route == "click_supersedes_request" {
        ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
        let clicked = branch.join("a-078");
        ctx.enable_accesskit();
        tree_frame(&mut app, &ctx, &mut frame, false);
        let id = ctx
            .data(|data| {
                data.get_temp::<egui::Id>(egui::Id::unique((
                    "folder_pane_test_label_id",
                    clicked.as_path(),
                )))
            })
            .unwrap();
        // Activate the actual row response. This isolates request ownership
        // from pointer hit testing and the native content-drag recognizer.
        app.folder_pane.set_cursor(root.clone());
        tree_event_frame(
            &mut app,
            &ctx,
            &mut frame,
            vec![egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Click,
                    target_tree: egui::accesskit::TreeId::ROOT,
                    target_node: id.accesskit_id(),
                    data: None,
                },
            )],
        );
        assert!(
            app.folder_pane
                .visible_rows()
                .iter()
                .any(|row| row.path == clicked && row.is_cursor)
        );
        assert_eq!(
            app.folder_pane.cursor_scroll,
            FolderPaneCursorScroll::Requested,
            "the old root reveal must leave the new click request with the owner"
        );
        for _ in 0..3 {
            tree_frame(&mut app, &ctx, &mut frame, false);
        }
        assert!(
            app.folder_pane
                .visible_rows()
                .iter()
                .any(|row| row.path == clicked && row.is_cursor)
        );
        let rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::unique((
                    "folder_pane_test_row_rect",
                    clicked.as_path(),
                )))
            })
            .unwrap();
        let viewport = ctx
            .data(|data| data.get_temp::<egui::Rect>(egui::Id::unique("folder_pane_test_viewport")))
            .unwrap();
        assert!(
            rect.top() >= viewport.top() && rect.bottom() <= viewport.bottom(),
            "new row click must supersede the old root reveal: {rect:?} / {viewport:?}"
        );
    }
    if route.starts_with("reopen_changed_during_scroll") {
        app.folder_pane.set_cursor(root.clone());
        for _ in 0..90 {
            tree_frame(&mut app, &ctx, &mut frame, false);
        }
        app.set_folder_tree_pane_visible(false);
        let changed_parent = if route.ends_with("branch") {
            &branch
        } else {
            &root
        };
        for index in 0..160 {
            std::fs::create_dir(changed_parent.join(format!("b-{index:03}"))).unwrap();
        }
        let release = app.folder_pane.test_hold_next_scan(changed_parent.clone());
        app.set_folder_tree_pane_visible(true);
        // Begin the real cursor scroll against the retained cache, then permit
        // the real refresh to insert preceding siblings during that scroll.
        tree_frame(&mut app, &ctx, &mut frame, false);
        assert!(app.folder_pane.has_pending());
        assert_eq!(
            app.folder_pane.cursor_scroll,
            FolderPaneCursorScroll::FollowingRefresh
        );
        let first_scrolls = tree_scrolls(&ctx);
        for _ in 0..4 {
            tree_frame(&mut app, &ctx, &mut frame, false);
        }
        assert_eq!(
            tree_scrolls(&ctx),
            first_scrolls,
            "unchanged pending layout must not force scrolling"
        );
        if route.contains("wheel") {
            let disabled = route.ends_with("wheel_disabled");
            let inside = !route.ends_with("wheel_outside");
            let pos = if inside {
                egui::pos2(100.0, 200.0)
            } else {
                egui::pos2(700.0, 200.0)
            };
            app.show_preferences = disabled;
            tree_event_frame(
                &mut app,
                &ctx,
                &mut frame,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, 100.0),
                        modifiers: egui::Modifiers::NONE,
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Unknown,
                    },
                ],
            );
            assert_eq!(
                app.folder_pane.cursor_scroll,
                if inside && !disabled {
                    FolderPaneCursorScroll::Idle
                } else {
                    FolderPaneCursorScroll::FollowingRefresh
                }
            );
            if disabled {
                // Keep the modal gate in place while the scan finishes and the
                // unconsumed disabled wheel tail decays. This fixture does not
                // draw the dialog which would normally consume that wheel.
                release.send(()).unwrap();
                assert_tree_location(&mut app, &ctx, &mut frame, &active);
                app.show_preferences = false;
                assert_tree_location(&mut app, &ctx, &mut frame, &active);
                return;
            }
            if inside && !disabled {
                let manual_scrolls = tree_scrolls(&ctx);
                release.send(()).unwrap();
                settle_tree_scans(&mut app, &ctx, &mut frame);
                assert_eq!(
                    tree_scrolls(&ctx),
                    manual_scrolls,
                    "manual tree scroll owns the viewport after refresh"
                );
                return;
            }
        }
        release.send(()).unwrap();
        assert_tree_location(&mut app, &ctx, &mut frame, &active);
    }
    if matches!(route, "reopen_moved" | "reopen_same") {
        app.set_folder_tree_pane_visible(false);
        tree_frame(&mut app, &ctx, &mut frame, false);
        let next = if route == "reopen_moved" {
            let next = root.join("z-other").join("z-current");
            std::fs::create_dir_all(&next).unwrap();
            app.current_folder = Some(next.clone());
            tree_frame(&mut app, &ctx, &mut frame, false);
            next
        } else {
            active
        };
        app.set_folder_tree_pane_visible(true);
        assert_tree_location(&mut app, &ctx, &mut frame, &next);
    }
}

#[test]
fn folder_pane_open_hidden_reveals_current_location() {
    tree_open_scenario("menu");
}
#[test]
fn folder_pane_open_key_reveals_current_location() {
    tree_open_scenario("key");
}
#[test]
fn folder_pane_open_startup_visible_reveals_current_location() {
    tree_open_scenario("startup_visible");
}
#[test]
fn folder_pane_open_reopen_after_hidden_navigation_reveals_current_location() {
    tree_open_scenario("reopen_moved");
}
#[test]
fn folder_pane_open_reopen_unchanged_reveals_current_location() {
    tree_open_scenario("reopen_same");
}
#[test]
fn folder_pane_open_reopen_refresh_during_scroll_reveals_current_location() {
    tree_open_scenario("reopen_changed_during_scroll");
}
#[test]
fn folder_pane_open_reopen_parent_refresh_during_scroll_reveals_current_location() {
    tree_open_scenario("reopen_changed_during_scroll_branch");
}
#[test]
fn folder_pane_open_tree_wheel_cancels_refresh_following() {
    tree_open_scenario("reopen_changed_during_scroll_wheel_inside");
}
#[test]
fn folder_pane_open_grid_wheel_preserves_refresh_following() {
    tree_open_scenario("reopen_changed_during_scroll_wheel_outside");
}
#[test]
fn folder_pane_open_disabled_tree_wheel_preserves_refresh_following() {
    tree_open_scenario("reopen_changed_during_scroll_wheel_disabled");
}
#[test]
fn folder_pane_open_new_row_click_supersedes_captured_request() {
    tree_open_scenario("click_supersedes_request");
}

#[test]
fn folder_pane_reported_delete_is_reflected_after_main_list_acceptance_without_tree_input() {
    let (mut app, folder, ctx) = fixture(&["a.jpg"]);
    let aaa = folder.join("aaa");
    std::fs::create_dir(&aaa).unwrap();
    app.load_folder(folder.clone());
    settle(&mut app, &ctx);
    app.folder_pane = FolderPaneState::test_tree(&folder, vec![aaa.clone()]);
    app.folder_pane.test_seed_listing(&aaa, vec![], true);
    assert!(
        app.items
            .iter()
            .any(|item| matches!(item,GridItem::Folder(path) if path==&aaa))
    );
    std::fs::remove_dir(&aaa).unwrap();
    app.start_external_rescan(&ctx, folder.clone(), SystemTime::UNIX_EPOCH);
    settle(&mut app, &ctx);
    assert!(
        !app.items
            .iter()
            .any(|item| matches!(item,GridItem::Folder(path) if path==&aaa))
    );
    assert!(app.folder_pane.test_unverified(&folder));
    // Only the ordinary completion pump runs; no tree click, reopen or reload.
    app.folder_pane.test_settle();
    assert!(app.folder_pane.test_children(&folder).is_empty());
    assert!(
        !app.folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == aaa)
    );
}

#[test]
fn folder_pane_only_accepted_equal_and_different_main_observations_publish_hints() {
    for route in ["equal", "different", "defer", "failure", "stale"] {
        let (mut app, folder, ctx) = fixture(&["a.jpg"]);
        app.folder_pane = FolderPaneState::test_tree(&folder, vec![]);
        let generation = app.items_generation;
        if matches!(route, "different" | "defer") {
            std::fs::write(folder.join("b.jpg"), b"new").unwrap();
        }
        if route == "defer" {
            app.fullscreen_idx = Some(0);
            app.viewer_presentation = ViewerPresentation::Fullscreen;
        }
        app.start_external_rescan(&ctx, folder.clone(), SystemTime::UNIX_EPOCH);
        let pending = app
            .external_rescan_pending
            .as_mut()
            .expect("real rescan request");
        if route == "stale" {
            pending.items_generation = pending.items_generation.wrapping_sub(1);
        }
        if route == "failure" {
            let (tx, rx) = mpsc::channel();
            tx.send(None).unwrap();
            pending.rx = rx;
        }
        settle(&mut app, &ctx);
        assert_eq!(
            app.folder_pane.test_unverified(&folder),
            matches!(route, "equal" | "different"),
            "{route}"
        );
        if route == "equal" {
            assert_eq!(app.items_generation, generation);
        }
        if route == "defer" {
            assert!(app.folder_refresh_pending.is_some());
            assert_eq!(app.items_generation, generation);
        }
    }
}

#[cfg(windows)]
#[test]
fn folder_pane_parked_main_acceptance_only_invalidates_main_pane_not_independent_context() {
    let (mut app, folder, ctx) = fixture(&["a.jpg"]);
    app.folder_pane = FolderPaneState::test_tree(&folder, vec![]);
    let main = app.viewer_context_main();
    let independent = app.fork_materialized_still_context(&folder, 0);
    app.with_viewer_context(independent, |app| {
        let generation = app.items_generation;
        let items = app.items.as_ptr();
        let cancel = app.cancel_token.clone();
        // Non-main active operations may reconcile, but must never notify the main pane.
        app.start_external_rescan_with_cause(
            &ctx,
            folder.clone(),
            SystemTime::UNIX_EPOCH,
            book_append_refresh::FolderRefreshCause::Operation,
        );
        settle(app, &ctx);
        assert!(!app.folder_pane.test_unverified(&folder));
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.items.as_ptr(), items);
        app.with_viewer_context(main, |app| {
            app.start_external_rescan(&ctx, folder.clone(), SystemTime::UNIX_EPOCH);
            settle(app, &ctx);
            assert!(app.folder_pane.test_unverified(&folder));
        })
        .unwrap();
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.items.as_ptr(), items);
        assert!(Arc::ptr_eq(&app.cancel_token, &cancel));
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(app.current_folder_watch.is_none());
    })
    .unwrap();
}

#[test]
fn folder_pane_main_return_rechecks_other_branch_even_when_list_a_is_equal() {
    for interruption in ["chrome_not_drawn", "tray", "minimized"] {
        let (mut app, a, ctx) = fixture(&["a.jpg"]);
        let root = a.parent().unwrap().to_path_buf();
        let b = root.join("B");
        std::fs::create_dir(&b).unwrap();
        app.settings.folder_tree_pane_visible = true;
        app.folder_pane = FolderPaneState::test_tree(&root, vec![a.clone(), b.clone()]);
        app.folder_pane.test_seed_listing(&b, vec![], true);
        match interruption {
            "chrome_not_drawn" => app.finish_folder_pane_presentation(&ctx, false),
            "tray" => {
                app.window_visible = false;
                app.service_folder_pane_visibility(&ctx);
            }
            _ => {
                let mut raw = egui::RawInput::default();
                raw.viewports
                    .get_mut(&egui::ViewportId::ROOT)
                    .unwrap()
                    .minimized = Some(true);
                ctx.run_ui(raw, |_| app.service_folder_pane_visibility(&ctx))
                    .drop_without_applying_deltas();
            }
        }
        std::fs::create_dir(b.join("new")).unwrap();
        let generation = app.items_generation;
        app.start_external_rescan(&ctx, a.clone(), SystemTime::UNIX_EPOCH);
        settle(&mut app, &ctx);
        assert_eq!(app.items_generation, generation, "A stayed Equal");
        assert!(app.folder_pane.test_children(&b).is_empty());
        app.window_visible = true;
        crate::ui_fonts::configure_fonts(&ctx);
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                app.render_folder_pane(ui);
            },
        )
        .drop_without_applying_deltas();
        app.folder_pane.test_settle();
        assert_eq!(
            app.folder_pane.test_children(&b),
            vec![b.join("new")],
            "{interruption}"
        );
        assert_eq!(app.items_generation, generation);
    }
}
