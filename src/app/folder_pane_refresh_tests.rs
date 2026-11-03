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
            app.post_render_main_navigation(ctx, empty_main_navigation());
            app.finish_folder_pane_presentation(ctx, true);
        },
    )
    .drop_without_applying_deltas();
}

fn complete_tree_frame(
    app: &mut App,
    ctx: &egui::Context,
    frame: &mut u64,
    events: Vec<egui::Event>,
) {
    *frame += 1;
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 480.0),
            )),
            time: Some(*frame as f64 / 60.0),
            events,
            ..Default::default()
        },
        |ui| {
            app.handle_keyboard(ctx);
            assert!(app.render_toolbar(ui).0.is_none());
            app.render_folder_pane(ui);
            assert!(app.render_grid(ui).is_none());
            app.post_render_main_navigation(ctx, empty_main_navigation());
            app.finish_folder_pane_presentation(ctx, true);
        },
    )
    .drop_without_applying_deltas();
}

fn empty_main_navigation() -> update_phases::MainFrameNavigation {
    update_phases::MainFrameNavigation {
        fav_nav: None,
        toolbar_fav_nav: None,
        fullscreen_close_nav: None,
        keyboard_nav: None,
        keyboard_nav_from_fullscreen_close: false,
        gamepad_nav: None,
        address_nav: None,
        open_folder_nav: None,
        context_nav: None,
        folder_pane_nav: None,
        grid_nav: None,
    }
}

fn navigation_tree_frame(
    app: &mut App,
    ctx: &egui::Context,
    frame: &mut u64,
    events: Vec<egui::Event>,
) -> Option<Arc<AtomicBool>> {
    *frame += 1;
    let mut started = None;
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 600.0),
            )),
            time: Some(*frame as f64 / 60.0),
            events,
            ..Default::default()
        },
        |ui| {
            let keyboard_nav = app.handle_keyboard(ctx);
            let (toolbar_fav_nav, address_nav) = app.render_toolbar(ui);
            let folder_pane_nav = app.render_folder_pane(ui).0;
            let grid_nav = app.render_grid(ui);
            started = app
                .folder_pane_open_pending
                .as_ref()
                .map(|pending| pending.cancel.clone());
            app.post_render_main_navigation(
                ctx,
                update_phases::MainFrameNavigation {
                    keyboard_nav,
                    toolbar_fav_nav,
                    address_nav,
                    folder_pane_nav,
                    grid_nav,
                    ..empty_main_navigation()
                },
            );
            app.finish_folder_pane_presentation(ctx, true);
        },
    )
    .drop_without_applying_deltas();
    started
}

fn navigation_tree_fixture() -> (AppTestEnvForTest, egui::Context, u64, PathBuf, PathBuf) {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let root = app.tmp.path().join("nav-tree");
    let a = root.join("a");
    let b = root.join("b");
    let c = root.join("c");
    for path in [&a, &b, &c] {
        std::fs::create_dir_all(path).unwrap();
    }
    app.folder_pane.test_scan_subtree(root);
    app.load_folder(a.clone());
    settle(&mut app, &ctx);
    app.set_folder_tree_pane_visible(true);
    let mut frame = 0;
    assert_tree_location(&mut app, &ctx, &mut frame, &a);
    app.folder_pane.set_cursor(b.clone());
    (app, ctx, frame, b, c)
}

fn tree_key_event(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn folder_pane_frame_end_close_does_not_override_toolbar_favorite_navigation() {
    let (mut app, ctx, mut frame, b, c) = navigation_tree_fixture();
    app.folder_pane.set_focus_grid();
    app.settings.show_toolbar_favorites = true;
    app.settings.toolbar_favorites_collapsed = false;
    app.settings.toolbar_favorites_display = crate::settings::ToolbarSectionDisplay::Buttons;
    app.settings.favorites = vec![crate::settings::FavoriteEntry::new(
        "C".to_owned(),
        c.clone(),
    )];
    for _ in 0..3 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    }
    let rect: egui::Rect = ctx
        .data(|data| data.get_temp(egui::Id::unique(("folder_pane_test_favorite_rect", &c))))
        .unwrap();
    let pos = rect.center();
    let pointer = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), pointer(true)],
    );
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![pointer(false), tree_key_event(egui::Key::F)],
    );
    assert!(!app.settings.folder_tree_pane_visible);
    assert!(
        app.folder_pane_open_pending.is_none(),
        "closing must not start cursor Open({b:?}) after the favorite won"
    );
    settle(&mut app, &ctx);
    for _ in 0..3 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    }
    assert_eq!(app.current_folder.as_ref(), Some(&c));
}

#[test]
fn folder_pane_frame_end_enter_then_f_preserves_the_first_open_worker() {
    let (mut app, ctx, mut frame, b, _) = navigation_tree_fixture();
    let first = navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            tree_key_event(egui::Key::Enter),
            tree_key_event(egui::Key::F),
        ],
    )
    .expect("Enter starts an Open");
    assert!(!app.settings.folder_tree_pane_visible);
    assert!(
        !first.load(Ordering::Relaxed),
        "F must not cancel Enter's worker"
    );
    if let Some(pending) = &app.folder_pane_open_pending {
        assert_eq!(pending.path, b);
        assert!(
            Arc::ptr_eq(&first, &pending.cancel),
            "F must not replace an already-completed Enter worker either"
        );
    }
}

#[test]
fn folder_pane_frame_end_new_cursor_commit_replaces_a_different_pending_open() {
    let (mut app, ctx, mut frame, b, c) = navigation_tree_fixture();
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    let label_id: egui::Id = ctx
        .data(|data| data.get_temp(egui::Id::unique(("folder_pane_test_label_id", b.as_path()))))
        .unwrap();
    let rect = ctx.read_response(label_id).unwrap().rect;
    let viewport: egui::Rect = ctx
        .data(|data| data.get_temp(egui::Id::unique("folder_pane_test_viewport")))
        .unwrap();
    let clickable = rect.intersect(viewport);
    assert!(clickable.is_positive(), "B's label must be in view");
    let pos = clickable.center();
    let pointer = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), pointer(true)],
    );
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![pointer(false)]);
    let pending = app
        .folder_pane_open_pending
        .as_mut()
        .expect("B click starts Open");
    assert_eq!(pending.path, b);
    let first = pending.cancel.clone();
    // Hold B's reply at the existing receiver boundary, independently of scan
    // timing. This is the same delayed-completion fixture used by refresh tests.
    let (_held_reply, rx) = mpsc::channel();
    pending.rx = rx;
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![tree_key_event(egui::Key::ArrowDown)],
    );
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == c && row.is_cursor)
    );
    assert!(!first.load(Ordering::Relaxed));
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![tree_key_event(egui::Key::F)],
    );
    assert!(!app.settings.folder_tree_pane_visible);
    assert!(
        first.load(Ordering::Relaxed),
        "F must cancel the older B request"
    );
    let pending = app
        .folder_pane_open_pending
        .as_ref()
        .expect("F starts C Open");
    assert_eq!(pending.path, c);
    assert!(!Arc::ptr_eq(&first, &pending.cancel));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        settle(&mut app, &ctx);
        if app.folder_pane_open_pending.is_none() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "C Open did not finish"
        );
        std::thread::yield_now();
    }
    assert_eq!(app.current_folder.as_ref(), Some(&c));
}

#[test]
fn folder_pane_frame_end_close_does_not_override_inline_search_drill() {
    for subfolder in [false, true] {
        let (mut app, ctx, _, _, c) = navigation_tree_fixture();
        app.global_search.active = true;
        if subfolder {
            app.drill_into_container(c.parent().unwrap().to_path_buf(), false);
        }
        app.request_folder_tree_pane_toggle(
            crate::ui_folder_pane::FolderPaneToggleKind::CommitCursorOnClose,
        );
        // These grid click handlers apply navigation immediately and return no
        // MainFrameNavigation candidate. Their surface owner retires the commit.
        if subfolder {
            app.drill_into_subfolder(c.clone());
        } else {
            app.drill_into_container(c.clone(), false);
        }
        app.post_render_main_navigation(&ctx, empty_main_navigation());
        app.finish_folder_pane_presentation(&ctx, true);
        assert!(!app.settings.folder_tree_pane_visible);
        assert!(app.folder_pane_open_pending.is_none());
        assert_eq!(
            app.global_search
                .drill
                .as_ref()
                .map(|drill| &drill.current_path),
            Some(&c)
        );
    }
}

#[test]
fn folder_pane_frame_end_without_navigation_arbitration_only_closes_visibility() {
    let (mut app, ctx, _, _, _) = navigation_tree_fixture();
    app.request_folder_tree_pane_toggle(
        crate::ui_folder_pane::FolderPaneToggleKind::CommitCursorOnClose,
    );
    app.finish_folder_pane_presentation(&ctx, false);
    assert!(!app.settings.folder_tree_pane_visible);
    assert!(!app.folder_pane.has_focus);
    assert!(app.folder_pane_open_pending.is_none());
    assert!(app.folder_pane_visibility_request.is_none());
}

#[test]
fn folder_pane_frame_end_close_does_not_override_inline_archive_admission() {
    use std::io::Write;
    for zipdir in [false, true] {
        let (mut app, ctx, _, _, _) = navigation_tree_fixture();
        let book = app.tmp.path().join("book.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&book).unwrap());
        writer
            .start_file("dir/001.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"image-source").unwrap();
        writer.finish().unwrap();
        app.request_folder_tree_pane_toggle(
            crate::ui_folder_pane::FolderPaneToggleKind::CommitCursorOnClose,
        );
        // Both handlers start an explicit main request directly, without a
        // MainFrameNavigation candidate or a folder-pane worker.
        if zipdir {
            app.open_rating_view_zipdir(book, "dir/".to_owned());
        } else {
            // Admission is checked through the request ownership below; this
            // regression does not consume the asynchronous archive payload.
            let _ = app.load_folder_or_convert_archive(book);
        }
        assert!(matches!(
            app.folder_pane_visibility_request,
            Some(crate::ui_folder_pane::FolderPaneVisibilityRequest::Hide)
        ));
        app.post_render_main_navigation(&ctx, empty_main_navigation());
        app.finish_folder_pane_presentation(&ctx, true);
        assert!(!app.settings.folder_tree_pane_visible);
        assert!(app.folder_pane_open_pending.is_none());
    }
}

#[test]
fn folder_pane_frame_end_automatic_refresh_retirement_preserves_cursor_commit() {
    let (mut app, ctx, _, b, _) = navigation_tree_fixture();
    app.request_folder_tree_pane_toggle(
        crate::ui_folder_pane::FolderPaneToggleKind::CommitCursorOnClose,
    );
    // Existing automatic refresh/continuation admission is not a new explicit
    // navigation and must not discard the user's pending cursor commit.
    app.retire_replaced_main_list_requests_with_startup_intent(None, false);
    app.post_render_main_navigation(&ctx, empty_main_navigation());
    app.finish_folder_pane_presentation(&ctx, true);
    assert!(!app.settings.folder_tree_pane_visible);
    assert_eq!(
        app.folder_pane_open_pending
            .as_ref()
            .map(|pending| &pending.path),
        Some(&b)
    );
}

fn frame_end_tree_toggle_scenario(case: &str) {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let root = app.tmp.path().join("frame-end-tree");
    let branch = root.join("z-branch");
    let active = branch.join("z-current");
    std::fs::create_dir_all(&active).unwrap();
    for parent in [&root, &branch] {
        for index in 0..80 {
            std::fs::create_dir(parent.join(format!("a-{index:03}"))).unwrap();
        }
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(8, 8)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    for index in 0..3 {
        std::fs::write(active.join(format!("img{index}.png")), png.get_ref()).unwrap();
    }
    app.folder_pane.test_scan_subtree(root);
    app.load_folder(active.clone());
    settle(&mut app, &ctx);
    app.settings.folder_tree_pane_visible = false;
    let mut frame = 0;
    if case == "toolbar_close_then_f" {
        app.set_folder_tree_pane_visible(true);
        assert_tree_location(&mut app, &ctx, &mut frame, &active);
        // Toolbar close never commits this explicit cursor. A temporary F close
        // caused by processing the packet backwards must not start an Open.
        app.folder_pane.set_cursor(branch.join("a-079"));
    }
    for _ in 0..3 {
        complete_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    }
    let toolbar: egui::Rect = ctx
        .data(|data| data.get_temp(egui::Id::unique("folder_pane_test_toggle_rect")))
        .unwrap();
    let pos = if case.starts_with("toolbar") {
        toolbar.center()
    } else {
        egui::pos2(900.0, 400.0)
    };
    let pointer = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let key = egui::Event::Key {
        key: egui::Key::F,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    if case != "key_only" && case != "accepted_f_then_arrow" {
        complete_tree_frame(
            &mut app,
            &ctx,
            &mut frame,
            vec![egui::Event::PointerMoved(pos), pointer(true)],
        );
    }
    if case == "accepted_f_then_arrow" {
        app.selected = Some(0);
    }
    let events = match case {
        "key_only" => vec![key],
        "accepted_f_then_arrow" => vec![
            key,
            egui::Event::Key {
                key: egui::Key::ArrowRight,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        "toolbar_only" => vec![pointer(false)],
        "accepted_f_then_grid_release" => vec![key, pointer(false)],
        _ => vec![pointer(false), key],
    };
    complete_tree_frame(&mut app, &ctx, &mut frame, events);
    if case == "accepted_f_then_arrow" {
        assert_eq!(app.selected, Some(1));
    }
    assert!(app.settings.folder_tree_pane_visible, "{case}");
    assert!(
        app.folder_pane.has_focus,
        "frame-end toggle must own focus: {case}"
    );
    assert!(
        app.folder_pane_open_pending.is_none(),
        "an intermediate close must not navigate: {case}"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        complete_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        if !app.folder_pane.has_pending() {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_tree_location(&mut app, &ctx, &mut frame, &active);
}

#[test]
fn folder_pane_frame_end_grid_release_then_f_reveals_and_focuses_current() {
    frame_end_tree_toggle_scenario("grid_then_f");
}

#[test]
fn folder_pane_frame_end_toolbar_close_then_f_opens_without_intermediate_navigation() {
    frame_end_tree_toggle_scenario("toolbar_close_then_f");
}

#[test]
fn folder_pane_frame_end_f_alone_reveals_current() {
    frame_end_tree_toggle_scenario("key_only");
}

#[test]
fn folder_pane_frame_end_toolbar_alone_reveals_current() {
    frame_end_tree_toggle_scenario("toolbar_only");
}

#[test]
fn folder_pane_accepted_frame_end_tradeoff_f_before_grid_release_keeps_tree_focus() {
    // User-approved limitation: all frame clicks run before the queued toggle,
    // including a grid release that physically followed F in the same packet.
    frame_end_tree_toggle_scenario("accepted_f_then_grid_release");
}

#[test]
fn folder_pane_accepted_frame_end_tradeoff_f_before_arrow_moves_grid() {
    // User-approved limitation: hidden F does not change routing until frame end.
    frame_end_tree_toggle_scenario("accepted_f_then_arrow");
}

#[test]
fn folder_pane_frame_end_composes_accepted_toggle_parity_without_intermediate_commit() {
    use crate::ui_folder_pane::FolderPaneToggleKind::{CommitCursorOnClose, VisibilityOnly};
    for shown in [false, true] {
        for count in 1..=3 {
            let mut app = setup_app_for_test();
            let ctx = egui::Context::default();
            let active = app.tmp.path().join("active");
            let other = active.join("other");
            std::fs::create_dir_all(&other).unwrap();
            app.current_folder = Some(active.clone());
            app.items_are_drive_list = false;
            app.folder_pane.test_scan_subtree(active.clone());
            app.set_folder_tree_pane_visible(shown);
            if shown {
                app.folder_pane.set_cursor(other.clone());
            }
            for _ in 0..count {
                app.request_folder_tree_pane_toggle(CommitCursorOnClose);
            }
            assert_eq!(
                app.settings.folder_tree_pane_visible, shown,
                "deferred until finish"
            );
            app.post_render_main_navigation(&ctx, empty_main_navigation());
            app.finish_folder_pane_presentation(&ctx, true);
            let visible = shown ^ (count % 2 != 0);
            assert_eq!(app.settings.folder_tree_pane_visible, visible);
            assert_eq!(app.folder_pane.has_focus, visible);
            assert!(app.folder_pane_visibility_request.is_none());
            if visible {
                // No drawing/scans have run: inspect the cursor directly via
                // Enter's read-only command, independently of cached rows.
                assert_eq!(
                    app.folder_pane
                        .handle_tree_key(crate::folder_pane::FolderPaneTreeKey::Enter),
                    Some(crate::folder_pane::FolderPaneCommand::Open(active.clone()))
                );
                assert!(app.folder_pane_open_pending.is_none());
            } else {
                assert_eq!(
                    app.folder_pane_open_pending
                        .as_ref()
                        .map(|pending| &pending.path),
                    shown.then_some(&other)
                );
            }
            app.finish_folder_pane_presentation(&ctx, true);
            assert_eq!(
                app.settings.folder_tree_pane_visible, visible,
                "consumed once"
            );
        }
    }
    // The final accepted handler determines close semantics (not raw event order).
    for last in [CommitCursorOnClose, VisibilityOnly] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let active = app.tmp.path().join("active");
        let other = active.join("other");
        std::fs::create_dir_all(&other).unwrap();
        app.current_folder = Some(active.clone());
        app.items_are_drive_list = false;
        app.folder_pane.test_scan_subtree(active);
        app.set_folder_tree_pane_visible(true);
        app.folder_pane.set_cursor(other);
        for kind in [VisibilityOnly, CommitCursorOnClose, last] {
            app.request_folder_tree_pane_toggle(kind);
        }
        app.post_render_main_navigation(&ctx, empty_main_navigation());
        app.finish_folder_pane_presentation(&ctx, true);
        assert!(!app.settings.folder_tree_pane_visible);
        assert_eq!(
            app.folder_pane_open_pending.is_some(),
            matches!(last, CommitCursorOnClose)
        );
    }
}

#[test]
fn folder_pane_frame_end_non_main_chrome_consumes_show_without_starting_scans() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let active = app.tmp.path().join("active");
    std::fs::create_dir_all(&active).unwrap();
    app.current_folder = Some(active);
    app.items_are_drive_list = false;
    app.settings.folder_tree_pane_visible = false;
    app.request_folder_tree_pane_toggle(
        crate::ui_folder_pane::FolderPaneToggleKind::CommitCursorOnClose,
    );
    app.finish_folder_pane_presentation(&ctx, false);
    assert!(app.settings.folder_tree_pane_visible);
    assert!(app.folder_pane.has_focus);
    assert!(app.folder_pane_visibility_request.is_none());
    assert!(!app.folder_pane.has_pending());
}

#[test]
fn folder_pane_frame_end_gamepad_accept_closes_without_restarting_open() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let active = app.tmp.path().join("active");
    let other = active.join("other");
    std::fs::create_dir_all(&other).unwrap();
    app.current_folder = Some(active.clone());
    app.items_are_drive_list = false;
    app.folder_pane.test_scan_subtree(active);
    app.set_folder_tree_pane_visible(true);
    app.folder_pane.set_cursor(other.clone());
    app.request_folder_tree_pane_toggle(
        crate::ui_folder_pane::FolderPaneToggleKind::CommitCursorOnClose,
    );
    assert!(app.handle_gamepad_grid_accept(&ctx).is_none());
    let cancel = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    app.finish_folder_pane_presentation(&ctx, true);
    assert!(!app.settings.folder_tree_pane_visible);
    assert!(!app.folder_pane.has_focus);
    let pending = app.folder_pane_open_pending.as_ref().unwrap();
    assert_eq!(pending.path, other);
    assert!(std::sync::Arc::ptr_eq(&cancel, &pending.cancel));
    assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert!(app.folder_pane_visibility_request.is_none());
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
    assert!(app.settings.folder_tree_pane_visible && app.folder_pane.has_focus);
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
