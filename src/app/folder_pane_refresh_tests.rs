use super::section375_tests::{fixture, settle};
use super::*;
use crate::folder_pane::{FolderPaneCursorScroll, FolderPaneState};
use std::time::SystemTime;

// These UI fixtures represent the main pane after startup admission. The
// headless App constructor never drives NotStarted into a terminal indexer
// outcome, so explicitly select the no-indexer outcome at fixture creation.
// Do not override a real Ready/Pending owner or any later hidden-window state.
fn prepare_headless_folder_pane_fixture(app: &mut App) {
    if matches!(app.indexer_init, IndexerInit::NotStarted) {
        app.indexer_init = IndexerInit::Unavailable;
    }
    assert!(
        !app.indexer_init.overlay_active(),
        "tree UI fixture must be past the startup overlay"
    );
}

#[cfg(windows)]
#[test]
fn folder_pane_user_search_click_and_enter_adopt_then_history_back_to_pre_search() {
    for kind in 0..4 {
        for enter in [false, true] {
            let mut app = App::navigation_search_fixture_for_test(kind);
            prepare_headless_folder_pane_fixture(&mut app);
            let origin = app.tmp.path().join("pre-search");
            let target = app.tmp.path().join("pane-user-target");
            std::fs::create_dir_all(&origin).unwrap();
            std::fs::create_dir(&target).unwrap();
            let root = app.tmp.path().to_path_buf();
            app.set_folder_tree_pane_visible(true);
            if kind == 1 {
                // adopt_everything_for_test reserves the temporary search editor's
                // first focus. This isolated pane frame does not draw that bar to
                // acknowledge the user's move to the tree. Retire only its editor
                // reservation through the existing focus-loss/navigation API; the
                // accepted dataset and producer/modal state are left intact.
                assert!(app.global_search.pending.is_none());
                assert!(app.global_search.focus_request || app.global_search.has_focus);
                app.retire_search_input_focus(
                    crate::ui_pinned_search::PinnedSearchKind::Everything,
                );
                assert!(!app.global_search.focus_request && !app.global_search.has_focus);
            }
            app.folder_pane =
                FolderPaneState::test_tree(&root, vec![origin.clone(), target.clone()]);
            app.folder_pane.test_seed_listing(&target, vec![], false);
            app.folder_pane.set_cursor(origin.clone());
            let ctx = egui::Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            ctx.enable_accesskit();
            ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
            let mut frame = 0;
            navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
            assert!(app.global_search.active || app.favsearch.active || app.tag_view.active);
            if enter {
                app.folder_pane.set_focus_tree();
                app.folder_pane.set_cursor(target.clone());
                assert!(
                    app.folder_pane_blocks_grid_keyboard(),
                    "kind={kind}: tree owns Enter"
                );
                assert!(
                    !app.everything_search_progress_visible(),
                    "kind={kind}: Enter must not be a new search submission"
                );
                assert!(
                    !app.main_chrome_pointer_blocked(),
                    "kind={kind}: pane has no modal owner"
                );
                assert!(
                    matches!(app.folder_pane.handle_tree_key(crate::folder_pane::FolderPaneTreeKey::Enter), Some(crate::folder_pane::FolderPaneCommand::Open(path)) if path == target),
                    "kind={kind}: real cursor resolves the target"
                );
                navigation_tree_frame(
                    &mut app,
                    &ctx,
                    &mut frame,
                    vec![tree_key_event(egui::Key::Enter)],
                );
            } else {
                let rect = label_click_rect(&ctx, &target);
                let viewport: egui::Rect = ctx
                    .data(|data| data.get_temp(egui::Id::unique("folder_pane_test_viewport")))
                    .unwrap();
                let clickable = rect.intersect(viewport);
                assert!(
                    clickable.is_positive(),
                    "kind={kind}: rendered target must be visible"
                );
                assert_ne!(app.folder_pane.active_path(), Some(target.as_path()));
                let pos = clickable.center();
                for pressed in [true, false] {
                    navigation_tree_frame(
                        &mut app,
                        &ctx,
                        &mut frame,
                        vec![
                            egui::Event::PointerMoved(pos),
                            egui::Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                    );
                }
            }
            if let Some(pending) = app.folder_pane_open_pending.as_ref() {
                assert_eq!(
                    pending.path, target,
                    "kind={kind} enter={enter}: actual handler requested path"
                );
                assert!(
                    matches!(
                        pending.purpose,
                        FolderOpenScanPurpose::JumpToPhysicalFolder { .. }
                    ),
                    "kind={kind} enter={enter}: user scan must exit search"
                );
            } else {
                assert_eq!(
                    app.current_folder.as_ref(),
                    Some(&target),
                    "kind={kind} enter={enter}: actual handler must start or finish navigation; pane_focus={} blocked={} modal={:?} cursor={:?}",
                    app.folder_pane.has_focus,
                    app.shortcuts_blocked_by_text_input(&ctx),
                    app.modal_dialog_block_reason(),
                    app.folder_pane
                        .visible_rows()
                        .iter()
                        .filter(|row| row.is_cursor)
                        .map(|row| row.path.clone())
                        .collect::<Vec<_>>()
                );
            }
            app.verify_admitted_user_navigation_for_test(&target, &origin);
            assert_eq!(
                app.current_folder.as_ref(),
                Some(&origin),
                "kind={kind} enter={enter}"
            );
            assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
        }
    }
}

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
            app.input_update_begin(ctx);
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
            app.input_update_begin(ctx);
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
    navigation_tree_frame_height(app, ctx, frame, events, 600.0)
}

fn navigation_tree_frame_height(
    app: &mut App,
    ctx: &egui::Context,
    frame: &mut u64,
    events: Vec<egui::Event>,
    height: f32,
) -> Option<Arc<AtomicBool>> {
    *frame += 1;
    let mut started = None;
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, height),
            )),
            time: Some(*frame as f64 / 60.0),
            events,
            ..Default::default()
        },
        |ui| {
            app.input_update_begin(ctx);
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
    prepare_headless_folder_pane_fixture(&mut app);
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let root = app.tmp.path().join("nav-tree");
    let a = root.join("a");
    let b = root.join("b");
    let c = root.join("c");
    for path in [&a, &b, &c] {
        std::fs::create_dir_all(path).unwrap();
    }
    app.folder_pane.test_scan_subtree(root.clone());
    app.load_folder(a.clone());
    settle(&mut app, &ctx);
    app.set_folder_tree_pane_visible(true);
    // Navigation arbitration needs only this subtree. Host TEMP ancestors can
    // scroll the 800x360 setup and leave a fading scrollbar over B's clipped
    // label when navigation_tree_frame switches to 1600x600. Fix the tree's
    // scope instead of waiting for unrelated scroll/layout state to settle.
    app.folder_pane = FolderPaneState::test_tree(&root, vec![a.clone(), b.clone(), c.clone()]);
    app.folder_pane.test_scan_subtree(root);
    app.folder_pane.set_cursor(a.clone());
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

fn scrollable_tree_click_fixture(
    expanded: bool,
) -> (AppTestEnvForTest, egui::Context, u64, PathBuf) {
    let mut app = setup_app_for_test();
    prepare_headless_folder_pane_fixture(&mut app);
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let root = app.tmp.path().join("click-scroll-tree");
    let children: Vec<_> = (0..80).map(|i| root.join(format!("row-{i:03}"))).collect();
    for child in &children {
        std::fs::create_dir_all(child).unwrap();
    }
    let clicked = children[34].clone();
    let grandchild = clicked.join("child");
    std::fs::create_dir(&grandchild).unwrap();
    app.load_folder(children[40].clone());
    settle(&mut app, &ctx);
    app.set_folder_tree_pane_visible(true);
    app.folder_pane = FolderPaneState::test_tree(&root, children.clone());
    app.folder_pane.test_scan_subtree(root);
    app.folder_pane
        .test_seed_listing(&clicked, vec![grandchild], expanded);
    app.folder_pane.set_cursor(children[40].clone());
    let mut frame = 0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        if !app.folder_pane.has_pending() {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    for _ in 0..3 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    }
    let rect = label_click_rect(&ctx, &clicked);
    let viewport = tree_click_viewport(&ctx);
    assert!(viewport.contains_rect(rect), "click a fully visible label");
    assert!((rect.center().y - viewport.center().y).abs() > 100.0);
    assert!(pane_scroll_state(&ctx).unclamped_offset().y > 100.0);
    assert_eq!(app.folder_pane.cursor_scroll, FolderPaneCursorScroll::Idle);
    (app, ctx, frame, clicked)
}

fn tree_click_viewport(ctx: &egui::Context) -> egui::Rect {
    ctx.data(|data| data.get_temp(egui::Id::unique("folder_pane_test_viewport")))
        .unwrap()
}

fn label_click_rect(ctx: &egui::Context, path: &Path) -> egui::Rect {
    // After run_ui, this_pass retains the older widget table while prev_pass
    // holds the just-completed one. read_response prefers this_pass, so stable
    // IDs can return stale geometry here. Use the actual draw's captured Rect.
    ctx.data(|data| data.get_temp(egui::Id::unique(("folder_pane_test_label_rect", path))))
        .expect("actual rendered label allocation")
}

fn pointer_tree_click(app: &mut App, ctx: &egui::Context, frame: &mut u64, pos: egui::Pos2) {
    for pressed in [true, false] {
        navigation_tree_frame(
            app,
            ctx,
            frame,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}

fn assert_tree_click_position(ctx: &egui::Context, path: &Path, offset: f32, rect: egui::Rect) {
    let actual_offset = pane_scroll_state(ctx).unclamped_offset().y;
    assert!(
        (actual_offset - offset).abs() < 0.1,
        "clicked row must not scroll: {actual_offset} != {offset}"
    );
    let actual = label_click_rect(ctx, path);
    assert!(
        (actual.top() - rect.top()).abs() < 0.1,
        "clicked row must stay on screen at the same y: {actual:?} != {rect:?}"
    );
}

#[test]
fn folder_pane_pointer_label_click_keeps_scroll_through_navigation_completion() {
    let (mut app, ctx, mut frame, clicked) = scrollable_tree_click_fixture(false);
    let rect = label_click_rect(&ctx, &clicked);
    let offset = pane_scroll_state(&ctx).unclamped_offset().y;
    let paths: Vec<_> = app
        .folder_pane
        .visible_rows()
        .into_iter()
        .map(|row| row.path)
        .collect();
    let scrolls = tree_scrolls(&ctx);
    pointer_tree_click(&mut app, &ctx, &mut frame, rect.center());
    let pending = app
        .folder_pane_open_pending
        .as_mut()
        .expect("label starts navigation");
    assert_eq!(pending.path, clicked);
    // Keep the real worker reply, but delay its adoption at the existing receiver
    // boundary so both pending frames and the actual completion are checked.
    let (_held_reply, rx) = mpsc::channel();
    let actual_reply = std::mem::replace(&mut pending.rx, rx);
    assert!(app.folder_pane.has_focus);
    for _ in 0..4 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        assert_tree_click_position(&ctx, &clicked, offset, rect);
    }
    app.folder_pane_open_pending.as_mut().unwrap().rx = actual_reply;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        settle(&mut app, &ctx);
        assert_tree_click_position(&ctx, &clicked, offset, rect);
        if app.folder_pane_open_pending.is_none() && !app.folder_pane.has_pending() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "clicked navigation did not finish"
        );
        std::thread::yield_now();
    }
    assert_eq!(app.current_folder.as_ref(), Some(&clicked));
    for _ in 0..4 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        assert_tree_click_position(&ctx, &clicked, offset, rect);
    }
    assert_eq!(
        app.folder_pane
            .visible_rows()
            .into_iter()
            .map(|row| row.path)
            .collect::<Vec<_>>(),
        paths
    );
    assert_eq!(
        tree_scrolls(&ctx),
        scrolls,
        "completion must not add a reveal"
    );
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == clicked && row.is_cursor && row.is_active)
    );
}

fn folder_pane_pointer_arrow_keeps_scroll(expanded: bool) {
    let (mut app, ctx, mut frame, clicked) = scrollable_tree_click_fixture(expanded);
    let rect = label_click_rect(&ctx, &clicked);
    let offset = pane_scroll_state(&ctx).unclamped_offset().y;
    let scrolls = tree_scrolls(&ctx);
    let active = app.current_folder.clone();
    // The 24pt arrow ends one item-spacing before the rendered label.
    let pos = egui::pos2(
        rect.left() - ctx.global_style().spacing.item_spacing.x - 12.0,
        rect.center().y,
    );
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    assert!(app.folder_pane.has_focus);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        assert_tree_click_position(&ctx, &clicked, offset, rect);
        if !app.folder_pane.has_pending() {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    for _ in 0..4 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        assert_tree_click_position(&ctx, &clicked, offset, rect);
    }
    let rows = app.folder_pane.visible_rows();
    let row = rows.iter().find(|row| row.path == clicked).unwrap();
    assert_eq!(row.expanded, !expanded);
    assert!(row.is_cursor);
    assert_eq!(
        rows.iter().any(|row| row.path == clicked.join("child")),
        !expanded
    );
    assert_eq!(app.current_folder, active, "arrow must not navigate");
    assert!(app.folder_pane_open_pending.is_none());
    assert_eq!(tree_scrolls(&ctx), scrolls);
}

#[test]
fn folder_pane_pointer_expand_click_keeps_scroll() {
    folder_pane_pointer_arrow_keeps_scroll(false);
}

#[test]
fn folder_pane_pointer_collapse_click_keeps_scroll() {
    folder_pane_pointer_arrow_keeps_scroll(true);
}

fn pointer_click_retires_waiting_cursor_request(arrow: bool, expanded: bool) {
    let (mut app, ctx, mut frame, clicked) = scrollable_tree_click_fixture(expanded);
    let waiting_parent = clicked.parent().unwrap().join("row-050");
    let waiting_cursor = waiting_parent.join("waiting-child");
    std::fs::create_dir(&waiting_cursor).unwrap();
    let release = app.folder_pane.test_hold_next_scan(waiting_parent.clone());
    app.folder_pane.set_cursor(waiting_parent);
    let _ = app
        .folder_pane
        .handle_tree_key(crate::folder_pane::FolderPaneTreeKey::Right);
    app.folder_pane.set_cursor(waiting_cursor.clone());
    let rect = label_click_rect(&ctx, &clicked);
    let offset = pane_scroll_state(&ctx).unclamped_offset().y;
    let scrolls = tree_scrolls(&ctx);
    // The expanded parent is still awaiting its first listing. Its child cursor
    // cannot be drawn, so Requested must survive multiple real pane frames.
    for _ in 0..2 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        assert!(app.folder_pane.has_pending());
        assert!(
            !app.folder_pane
                .visible_rows()
                .iter()
                .any(|row| row.path == waiting_cursor)
        );
        assert_eq!(
            app.folder_pane.cursor_scroll,
            FolderPaneCursorScroll::Requested
        );
        assert_tree_click_position(&ctx, &clicked, offset, rect);
    }
    let pos = if arrow {
        egui::pos2(
            rect.left() - ctx.global_style().spacing.item_spacing.x - 12.0,
            rect.center().y,
        )
    } else {
        rect.center()
    };
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let row = app
        .folder_pane
        .visible_rows()
        .into_iter()
        .find(|row| row.path == clicked)
        .unwrap();
    assert!(
        row.is_cursor,
        "the actual pointer handler must replace the missing cursor"
    );
    if arrow {
        assert_eq!(row.expanded, !expanded);
        assert!(app.folder_pane_open_pending.is_none());
    } else {
        assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, clicked);
    }
    for _ in 0..4 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        assert_tree_click_position(&ctx, &clicked, offset, rect);
        assert_eq!(app.folder_pane.cursor_scroll, FolderPaneCursorScroll::Idle);
    }
    release.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        settle(&mut app, &ctx);
        assert_tree_click_position(&ctx, &clicked, offset, rect);
        if !app.folder_pane.has_pending() && app.folder_pane_open_pending.is_none() {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    for _ in 0..4 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        assert_tree_click_position(&ctx, &clicked, offset, rect);
    }
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == waiting_cursor)
    );
    assert_eq!(
        tree_scrolls(&ctx),
        scrolls,
        "the obsolete reveal must stay retired after the listing arrives"
    );
}

#[test]
fn folder_pane_pointer_label_click_retires_waiting_cursor_request() {
    pointer_click_retires_waiting_cursor_request(false, false);
}

#[test]
fn folder_pane_pointer_expand_click_retires_waiting_cursor_request() {
    pointer_click_retires_waiting_cursor_request(true, false);
}

#[test]
fn folder_pane_pointer_collapse_click_retires_waiting_cursor_request() {
    pointer_click_retires_waiting_cursor_request(true, true);
}

#[test]
fn folder_pane_pointer_click_then_keyboard_down_still_centers_cursor() {
    let (mut app, ctx, mut frame, clicked) = scrollable_tree_click_fixture(false);
    let rect = label_click_rect(&ctx, &clicked);
    let pos = egui::pos2(
        rect.left() - ctx.global_style().spacing.item_spacing.x - 12.0,
        rect.center().y,
    );
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let scrolls = tree_scrolls(&ctx);
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![tree_key_event(egui::Key::ArrowDown)],
    );
    for _ in 0..3 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    }
    let rows = app.folder_pane.visible_rows();
    let cursor = &rows.iter().find(|row| row.is_cursor).unwrap().path;
    assert_ne!(cursor, &clicked);
    assert!(
        (label_click_rect(&ctx, cursor).center().y - tree_click_viewport(&ctx).center().y).abs()
            < 4.0
    );
    assert!(
        tree_scrolls(&ctx) > scrolls,
        "keyboard must still request reveal"
    );
}

fn visible_label_click_rect(ctx: &egui::Context, path: &Path) -> egui::Rect {
    let viewport = ctx
        .data(|data| data.get_temp::<egui::Rect>(egui::Id::unique("folder_pane_test_viewport")))
        .unwrap();
    let allocation = label_click_rect(ctx, path);
    let rect = allocation.intersect(viewport);
    assert!(
        rect.is_positive(),
        "label must be visible: {path:?}, allocation={allocation:?}, viewport={viewport:?}, scroll={:?}",
        pane_scroll_state(ctx).unclamped_offset()
    );
    rect
}

// Unlike the older pane-only harness, this follows the root input/adoption/
// hydration order. The relay holds only the real checking worker's receive.
fn sidecar_tree_frame(
    app: &mut App,
    ctx: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
    attach: Option<std::rc::Rc<sidecar_restore::SidecarCheckingRelay>>,
) {
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 600.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ui| {
            // The real root logic phase drains App-global writer replies before
            // hydration polls its quiescence fence. In particular bookmark reads
            // requested by the first folder install must cross that receive edge.
            app.poll_book_bookmark_service_responses();
            app.poll_spread_shift_service();
            app.service_update_registered_writers(ctx);
            app.input_update_begin(ctx);
            app.service_edit_completions(ctx, std::time::Instant::now());
            app.service_initial_worker_results(ctx, &mut None);
            app.render_media_and_edit_results(ctx);
            app.render_search_and_metadata_results(ctx);
            app.render_other_worker_results(ctx);
            let keyboard_nav = app.handle_keyboard(ctx);
            app.render_menu_worker_results(ctx);
            let (toolbar_fav_nav, address_nav) = app.render_toolbar(ui);
            let folder_pane_nav = app.render_folder_pane(ui).0;
            let grid_nav = app.render_grid(ui);
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
            app.show_sidecar_restore_dialog(ctx);
            if let Some(relay) = attach.clone() {
                if app.sidecar_restore.is_some() {
                    app.attach_sidecar_checking_relay_for_test(relay);
                }
            }
            app.post_render_sidecar_restore(ctx);
        },
    )
    .drop_without_applying_deltas();
}

fn primary_event(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[derive(Clone, Copy)]
enum SidecarTreeFixtureMode {
    Name,
    NearArrow,
    PressOtherRowBeforeRestore,
}

struct SidecarTreeFixture {
    app: AppTestEnvForTest,
    ctx: egui::Context,
    b: PathBuf,
    c: PathBuf,
    pos: egui::Pos2,
    first: Arc<AtomicBool>,
    relay: std::rc::Rc<sidecar_restore::SidecarCheckingRelay>,
}

fn sidecar_tree_fixture(mode: SidecarTreeFixtureMode) -> SidecarTreeFixture {
    let (mut app, ctx, mut frame, b, c) = navigation_tree_fixture();
    app.active_quick_folder_slot = None;
    app.settings.sidecar_backup_enabled = true;
    app.settings.tag_sidecar_backup_enabled = false;
    app.settings.auto_fullscreen_image_folders = false;
    std::fs::create_dir(b.join("child")).unwrap();
    std::fs::write(b.join("page.jpg"), b"fixture").unwrap();
    let mut sidecar = crate::sidecar::SidecarFile::load(&b);
    sidecar.set_adjust(
        "page.jpg",
        crate::adjustment::AdjustParams {
            brightness: 12.0,
            ..Default::default()
        },
    );
    assert!(sidecar.flush_blocking());
    app.folder_pane
        .test_seed_listing(&b, vec![b.join("child")], false);
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    let rect = visible_label_click_rect(&ctx, &b);
    let pos = if matches!(mode, SidecarTreeFixtureMode::NearArrow) {
        egui::pos2(rect.left() + 0.1, rect.center().y)
    } else {
        rect.center()
    };
    // Fixed egui time is independent of disk/worker scheduling.
    let t = 10.0;
    sidecar_tree_frame(
        &mut app,
        &ctx,
        t,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, true)],
        None,
    );
    sidecar_tree_frame(
        &mut app,
        &ctx,
        t + 0.01,
        vec![primary_event(pos, false)],
        None,
    );
    let first = app
        .folder_pane_open_pending
        .as_ref()
        .expect("first real click starts navigation")
        .cancel
        .clone();
    let relay = sidecar_restore::SidecarCheckingRelay::new();
    if matches!(mode, SidecarTreeFixtureMode::PressOtherRowBeforeRestore) {
        let other = visible_label_click_rect(&ctx, &c).center();
        sidecar_tree_frame(
            &mut app,
            &ctx,
            t + 0.015,
            vec![egui::Event::PointerMoved(other), primary_event(other, true)],
            Some(relay.clone()),
        );
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.sidecar_restore.is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "real navigation must start sidecar restoration"
        );
        sidecar_tree_frame(&mut app, &ctx, t + 0.02, vec![], Some(relay.clone()));
        std::thread::yield_now();
    }
    while !relay.captured() {
        if std::time::Instant::now() >= deadline {
            eprintln!(
                "hydration writers={:?}",
                app.metadata_transfer_writers_busy_for_sidecar_restore(false)
            );
            eprintln!(
                "writer detail: rating_xmp={} tag_batch={} book_resume={} rating={} edit_preview={} local_adjust={} local_pending={} bookmarks={} spread={} favorite={}",
                app.rating_xmp_hydration.is_busy(),
                app.tag_write_handle
                    .as_ref()
                    .is_some_and(|h| h.has_unconsumed_batch()),
                app.book_resume_writer.as_ref().is_some_and(|w| w.is_busy()),
                app.rating_write_handle
                    .as_ref()
                    .is_some_and(|h| h.is_busy()),
                app.edit_preview_cache
                    .as_ref()
                    .is_some_and(|c| c.has_pending_commands()),
                app.local_adjust_write_handle
                    .as_ref()
                    .is_some_and(|h| h.has_unfinished_work()),
                app.local_adjust_write_pending.len(),
                app.book_bookmark_pending_requests.len(),
                app.spread_shift_service
                    .as_ref()
                    .is_some_and(|service| service.is_busy()),
                app.favorite_view_store_writer
                    .as_ref()
                    .is_some_and(|writer| writer.is_busy())
            );
            eprintln!(
                "pending detail: tag_maintenance={} rename_inflight={} rename_queue={} cleanup={} purge={} bookmark_delete={} drop_copy={} new_folder={} rename={} delete={} book_op={} capture={} export={} batch_convert={} transfer={} edit_bundles={:?}",
                app.tag_maintenance_rx.is_some(),
                app.rename_migration_in_flight.is_some(),
                app.rename_migration_queue.len(),
                app.metadata_cleanup_pending.is_some(),
                app.delete_purge_retry_pending.is_some(),
                app.bookmark_delete_pending.is_some(),
                app.drop_copy_pending.len(),
                app.new_folder_pending.is_some(),
                app.rename_pending.is_some(),
                app.delete_pending.is_some(),
                app.book_op_pending.is_some(),
                app.capture_pending.is_some(),
                app.export_pending.is_some(),
                app.batch_convert.is_some(),
                app.metadata_transfer_active_for_sidecar_restore(),
                (
                    app.edit_bundle_paste_pending.is_some(),
                    app.edit_bundle_apply_pending.is_some(),
                    app.edit_bundle_bulk_pending.is_some()
                )
            );
        }
        assert!(
            std::time::Instant::now() < deadline,
            "real sidecar checking worker must complete: {:?}; local writes={} identity_restore={}",
            app.sidecar_restore
                .as_ref()
                .map(|state| state.presentation()),
            app.local_adjust_write_pending.len(),
            app.content_identity_restore_pending.is_some()
        );
        sidecar_tree_frame(&mut app, &ctx, t + 0.03, vec![], None);
        std::thread::yield_now();
    }
    assert_eq!(app.current_folder.as_ref(), Some(&b));
    assert!(app.sidecar_restore_blocks_projected_context());
    assert!(!first.load(Ordering::Relaxed));
    assert_label_expanded(&app, &b, false);
    assert!(
        ctx.input(|input| input.pointer.latest_pos().is_none()),
        "the real recovery input gate must erase egui pointer history"
    );
    SidecarTreeFixture {
        app,
        ctx,
        b,
        c,
        pos,
        first,
        relay,
    }
}

fn finish_sidecar_tree(
    app: &mut App,
    ctx: &egui::Context,
    relay: &sidecar_restore::SidecarCheckingRelay,
    time: f64,
) {
    relay.release();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.sidecar_restore.is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "real sidecar hydration must resume"
        );
        sidecar_tree_frame(app, ctx, time, vec![], None);
        std::thread::yield_now();
    }
}

fn folder_pane_sidecar_second_click_scenario(timing: &str) {
    let SidecarTreeFixture {
        mut app,
        ctx,
        b,
        pos,
        first,
        relay,
        ..
    } = sidecar_tree_fixture(SidecarTreeFixtureMode::Name);
    let t = 10.0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    if timing == "during" || timing == "modal" {
        if timing == "modal" {
            app.sidecar_restore
                .as_mut()
                .unwrap()
                .elapse_modal_grace_for_test();
        } else {
            app.sidecar_restore
                .as_mut()
                .unwrap()
                .reset_modal_grace_for_test();
        }
        // Establish the modal layer (or its absence) in the previous-pass hit table.
        for time in [t + 0.05, t + 0.06] {
            sidecar_tree_frame(&mut app, &ctx, time, vec![], None);
        }
        assert_eq!(
            ctx.memory(|memory| memory.top_modal_layer().is_some()),
            timing == "modal"
        );
    }
    let request = app
        .sidecar_restore
        .as_ref()
        .unwrap()
        .connection_request_ids_for_test();
    let restore_cancel = app.sidecar_restore.as_ref().unwrap().cancel_for_test();
    let generation = app.items_generation;
    let reveals = tree_scrolls(&ctx);
    let focus = app.folder_pane.has_focus;
    let cursor = app
        .folder_pane
        .visible_rows()
        .into_iter()
        .find(|row| row.is_cursor)
        .map(|row| row.path);
    let reveal = app.folder_pane.cursor_scroll;
    let owner = app.keyboard_owner_for_pass(&ctx);
    if timing == "after" {
        relay.release();
        while app.sidecar_restore.is_some() {
            assert!(std::time::Instant::now() < deadline);
            sidecar_tree_frame(&mut app, &ctx, t + 0.04, vec![], None);
            std::thread::yield_now();
        }
    }
    let mut press = vec![egui::Event::PointerMoved(pos), primary_event(pos, true)];
    if timing == "during" || timing == "modal" {
        press.push(egui::Event::Text("discard during hydration".into()));
        press.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 40.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        });
    }
    sidecar_tree_frame(&mut app, &ctx, t + 0.08, press, None);
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == b && row.expanded),
        "second press must open the first row even {timing} sidecar hydration"
    );
    if let Some(state) = app.sidecar_restore.as_ref() {
        assert_eq!(state.connection_request_ids_for_test(), request);
        assert!(Arc::ptr_eq(&restore_cancel, &state.cancel_for_test()));
        assert!(!restore_cancel.load(Ordering::Relaxed));
        assert_eq!(app.keyboard_owner_for_pass(&ctx), owner);
        assert_eq!(app.folder_pane.has_focus, focus);
        assert_eq!(
            app.folder_pane
                .visible_rows()
                .into_iter()
                .find(|row| row.is_cursor)
                .map(|row| row.path),
            cursor
        );
        assert_eq!(app.folder_pane.cursor_scroll, reveal);
        assert_eq!(tree_scrolls(&ctx), reveals);
    }
    if timing == "across" {
        relay.release();
        while app.sidecar_restore.is_some() {
            assert!(std::time::Instant::now() < deadline);
            sidecar_tree_frame(&mut app, &ctx, t + 0.09, vec![], None);
            std::thread::yield_now();
        }
    }
    sidecar_tree_frame(
        &mut app,
        &ctx,
        t + 0.10,
        vec![primary_event(pos, false)],
        None,
    );
    assert!(!first.load(Ordering::Relaxed));
    assert!(
        app.folder_pane_open_pending.is_none(),
        "second click must never navigate again"
    );
    assert_eq!(app.folder_pane.has_focus, focus);
    assert_eq!(
        app.folder_pane
            .visible_rows()
            .into_iter()
            .find(|row| row.is_cursor)
            .map(|row| row.path),
        cursor
    );
    assert_eq!(app.folder_pane.cursor_scroll, reveal);
    assert_eq!(tree_scrolls(&ctx), reveals);
    assert_eq!(app.items_generation, generation);
    assert!(!restore_cancel.load(Ordering::Relaxed));
    if timing == "during" || timing == "modal" {
        assert_eq!(
            app.sidecar_restore
                .as_ref()
                .unwrap()
                .connection_request_ids_for_test(),
            request
        );
        assert_eq!(app.keyboard_owner_for_pass(&ctx), owner);
        assert!(Arc::ptr_eq(
            &restore_cancel,
            &app.sidecar_restore.as_ref().unwrap().cancel_for_test()
        ));
        ctx.input(|input| {
            // Recovery deliberately retains release/cancel transport while
            // discarding presses, Text and wheel input. This pass supplied one
            // release; requiring an empty queue would change that contract.
            assert_eq!(input.events.len(), 1);
            assert!(matches!(input.events[0], egui::Event::PointerButton {
                pos: released, button: egui::PointerButton::Primary, pressed: false, ..
            } if released == pos));
            assert_eq!(input.smooth_scroll_delta, egui::Vec2::ZERO);
            assert!(!input.pointer.any_down());
        });
        relay.release();
    }
    while app.sidecar_restore.is_some() {
        assert!(std::time::Instant::now() < deadline);
        sidecar_tree_frame(&mut app, &ctx, t + 0.11, vec![], None);
        std::thread::yield_now();
    }
    assert_eq!(app.current_folder.as_ref(), Some(&b));
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == b && row.expanded)
    );
}

#[test]
fn folder_pane_label_sidecar_second_press_during_restore() {
    folder_pane_sidecar_second_click_scenario("during");
}
#[test]
fn folder_pane_label_sidecar_second_press_across_restore() {
    folder_pane_sidecar_second_click_scenario("across");
}
#[test]
fn folder_pane_label_sidecar_second_press_after_restore() {
    folder_pane_sidecar_second_click_scenario("after");
}
#[test]
fn folder_pane_label_sidecar_second_press_under_real_restore_modal() {
    folder_pane_sidecar_second_click_scenario("modal");
}

fn sidecar_rejected_input_scenario(kind: &str) {
    let mode = match kind {
        "arrow-grace" | "arrow-modal" => SidecarTreeFixtureMode::NearArrow,
        "release-after-start" => SidecarTreeFixtureMode::PressOtherRowBeforeRestore,
        _ => SidecarTreeFixtureMode::Name,
    };
    let SidecarTreeFixture {
        mut app,
        ctx,
        b,
        c,
        pos,
        first,
        relay,
    } = sidecar_tree_fixture(mode);
    if kind == "arrow-modal" {
        app.sidecar_restore
            .as_mut()
            .unwrap()
            .elapse_modal_grace_for_test();
    } else {
        app.sidecar_restore
            .as_mut()
            .unwrap()
            .reset_modal_grace_for_test();
    }
    for time in [10.04, 10.05] {
        sidecar_tree_frame(&mut app, &ctx, time, vec![], None);
    }
    assert_eq!(
        ctx.memory(|memory| memory.top_modal_layer().is_some()),
        kind == "arrow-modal"
    );
    let request = app
        .sidecar_restore
        .as_ref()
        .unwrap()
        .connection_request_ids_for_test();
    let cancel = app.sidecar_restore.as_ref().unwrap().cancel_for_test();
    let focus = app.folder_pane.has_focus;
    let cursor = app
        .folder_pane
        .visible_rows()
        .into_iter()
        .find(|row| row.is_cursor)
        .map(|row| row.path);
    if kind == "other-modal" {
        app.show_preferences = true;
    }
    if kind == "release-after-start" {
        let other = visible_label_click_rect(&ctx, &c).center();
        sidecar_tree_frame(
            &mut app,
            &ctx,
            10.08,
            vec![primary_event(other, false)],
            None,
        );
    } else {
        let target = if kind.starts_with("arrow-") {
            egui::pos2(visible_label_click_rect(&ctx, &b).left() - 3.9, pos.y)
        } else if kind == "new-click" {
            visible_label_click_rect(&ctx, &c).center()
        } else {
            pos
        };
        if kind.starts_with("arrow-") {
            assert!(target.distance(pos) < 6.0);
        }
        sidecar_tree_frame(
            &mut app,
            &ctx,
            10.08,
            vec![
                egui::Event::PointerMoved(target),
                primary_event(target, true),
            ],
            None,
        );
        sidecar_tree_frame(
            &mut app,
            &ctx,
            10.09,
            vec![primary_event(target, false)],
            None,
        );
    }
    assert_label_expanded(&app, &b, false);
    assert_label_expanded(&app, &c, false);
    assert_eq!(app.folder_pane.has_focus, focus);
    assert_eq!(
        app.folder_pane
            .visible_rows()
            .into_iter()
            .find(|row| row.is_cursor)
            .map(|row| row.path),
        cursor
    );
    assert!(app.folder_pane_open_pending.is_none());
    assert!(!first.load(Ordering::Relaxed));
    assert!(!cancel.load(Ordering::Relaxed));
    assert_eq!(
        app.sidecar_restore
            .as_ref()
            .unwrap()
            .connection_request_ids_for_test(),
        request
    );
    app.show_preferences = false;
    finish_sidecar_tree(&mut app, &ctx, &relay, 10.11);
    assert_eq!(app.current_folder.as_ref(), Some(&b));
    assert_label_expanded(&app, &b, false);
    if kind.starts_with("arrow-") {
        // The disabled arrow ended the name gesture. A fresh name click at the
        // old position after recovery is a single click, even inside the delay.
        for time in [10.115, 10.12] {
            sidecar_tree_frame(&mut app, &ctx, time, vec![], None);
        }
        for (time, pressed) in [(10.13, true), (10.14, false)] {
            sidecar_tree_frame(
                &mut app,
                &ctx,
                time,
                vec![egui::Event::PointerMoved(pos), primary_event(pos, pressed)],
                None,
            );
        }
        assert_label_expanded(&app, &b, false);
        assert!(app.folder_pane_open_pending.is_none());
        sidecar_tree_frame(&mut app, &ctx, 10.15, vec![primary_event(pos, true)], None);
        assert_label_expanded(&app, &b, true);
    }
}

#[test]
fn folder_pane_label_sidecar_and_other_modal_reject_continuation() {
    sidecar_rejected_input_scenario("other-modal");
}
#[test]
fn folder_pane_label_release_after_restore_started_does_not_navigate() {
    sidecar_rejected_input_scenario("release-after-start");
}
#[test]
fn folder_pane_label_arrow_during_restore_grace_cancels_name_sequence() {
    sidecar_rejected_input_scenario("arrow-grace");
}
#[test]
fn folder_pane_label_arrow_under_restore_modal_cancels_name_sequence() {
    sidecar_rejected_input_scenario("arrow-modal");
}
#[test]
fn folder_pane_label_new_click_during_restore_is_discarded() {
    sidecar_rejected_input_scenario("new-click");
}

fn ordered_label_fixture() -> (
    AppTestEnvForTest,
    egui::Context,
    u64,
    PathBuf,
    PathBuf,
    egui::Pos2,
) {
    let (mut app, ctx, mut frame, b, c) = navigation_tree_fixture();
    std::fs::create_dir(b.join("child")).unwrap();
    app.folder_pane
        .test_seed_listing(&b, vec![b.join("child")], false);
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    let pos = visible_label_click_rect(&ctx, &b).center();
    (app, ctx, frame, b, c, pos)
}

fn assert_label_expanded(app: &App, path: &Path, expected: bool) {
    assert_eq!(
        app.folder_pane
            .visible_rows()
            .iter()
            .find(|row| row.path == path)
            .unwrap()
            .expanded,
        expected,
        "{path:?}"
    );
}

#[test]
fn folder_pane_label_ordered_two_clicks_in_one_pass() {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            egui::Event::PointerMoved(pos),
            primary_event(pos, true),
            primary_event(pos, false),
            primary_event(pos, true),
            primary_event(pos, false),
        ],
    );
    assert_label_expanded(&app, &b, true);
    assert_eq!(
        app.folder_pane_open_pending
            .as_ref()
            .expect("first click immediately navigates")
            .path,
        b
    );
}

#[test]
fn folder_pane_label_ordered_release_press_release_in_one_pass() {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, true)],
    );
    assert!(app.folder_pane_open_pending.is_none());
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            primary_event(pos, false),
            primary_event(pos, true),
            primary_event(pos, false),
        ],
    );
    assert_label_expanded(&app, &b, true);
    assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, b);
}

#[test]
fn folder_pane_label_ordered_repeat_then_distant_label_navigates() {
    let (mut app, ctx, mut frame, b, c, pos) = ordered_label_fixture();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let first = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    let (_hold, rx) = mpsc::channel();
    app.folder_pane_open_pending.as_mut().unwrap().rx = rx;
    let other = visible_label_click_rect(&ctx, &c).center();
    assert!(pos.distance(other) > 6.0);
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            primary_event(pos, true),
            primary_event(pos, false),
            egui::Event::PointerMoved(other),
            primary_event(other, true),
            primary_event(other, false),
        ],
    );
    assert_label_expanded(&app, &b, true);
    assert_label_expanded(&app, &c, false);
    assert!(first.load(Ordering::Relaxed));
    assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, c);
}

#[test]
fn folder_pane_label_ordered_drag_away_and_back_is_not_click() {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, true)],
    );
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos + egui::vec2(20.0, 0.0))],
    );
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, false)],
    );
    assert!(app.folder_pane_open_pending.is_none());
    assert_label_expanded(&app, &b, false);
}

fn accepted_label_release_scenario(long: bool) {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    let first = pending.cancel.clone();
    let (_hold, rx) = mpsc::channel();
    pending.rx = rx;
    let press = pos + egui::vec2(4.0, 0.0);
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(press), primary_event(press, true)],
    );
    assert_label_expanded(&app, &b, true);
    if long {
        frame += 180;
    }
    let release = if long {
        press
    } else {
        pos + egui::vec2(8.0, 0.0)
    };
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            egui::Event::PointerMoved(release),
            primary_event(release, false),
        ],
    );
    assert!(!first.load(Ordering::Relaxed));
    assert!(Arc::ptr_eq(
        &first,
        &app.folder_pane_open_pending.as_ref().unwrap().cancel
    ));
    assert_label_expanded(&app, &b, true);
}

#[test]
fn folder_pane_label_ordered_accepted_release_beyond_first_position_does_not_renavigate() {
    accepted_label_release_scenario(false);
}
#[test]
fn folder_pane_label_ordered_accepted_long_hold_does_not_renavigate() {
    accepted_label_release_scenario(true);
}

#[test]
fn folder_pane_label_ordered_key_cancels_before_second_press_and_preserves_new_sequence() {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let first = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    let (_hold, rx) = mpsc::channel();
    app.folder_pane_open_pending.as_mut().unwrap().rx = rx;
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            tree_key_event(egui::Key::ArrowDown),
            primary_event(pos, true),
            primary_event(pos, false),
        ],
    );
    assert_label_expanded(&app, &b, false);
    assert!(
        first.load(Ordering::Relaxed),
        "the key ends the old sequence, then the label navigates"
    );
    assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, b);
    let replacement = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    assert_label_expanded(&app, &b, true);
    assert!(
        !replacement.load(Ordering::Relaxed),
        "the post-key click starts a surviving new sequence"
    );
}

#[test]
fn folder_pane_label_ordered_key_then_first_click_uses_label_cursor() {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    app.folder_pane.has_focus = true;
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            tree_key_event(egui::Key::ArrowDown),
            egui::Event::PointerMoved(pos),
            primary_event(pos, true),
            primary_event(pos, false),
        ],
    );
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == b && row.is_cursor)
    );
    assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, b);
    assert_eq!(app.folder_pane.cursor_scroll, FolderPaneCursorScroll::Idle);
}

#[test]
fn folder_pane_label_touch_double_tap_navigates_without_toggle() {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    let touch = |phase| egui::Event::Touch {
        device_id: egui::TouchDeviceId(769),
        id: egui::TouchId(1),
        phase,
        pos,
        force: None,
    };
    let mut first = None;
    let mut holds = Vec::new();
    for _ in 0..2 {
        navigation_tree_frame(
            &mut app,
            &ctx,
            &mut frame,
            vec![
                touch(egui::TouchPhase::Start),
                egui::Event::PointerMoved(pos),
                primary_event(pos, true),
            ],
        );
        navigation_tree_frame(
            &mut app,
            &ctx,
            &mut frame,
            vec![
                touch(egui::TouchPhase::End),
                primary_event(pos, false),
                egui::Event::PointerGone,
            ],
        );
        assert_label_expanded(&app, &b, false);
        let pending = app
            .folder_pane_open_pending
            .as_mut()
            .expect("each tap remains a single navigation");
        assert_eq!(pending.path, b);
        if let Some(first) = first.as_ref() {
            assert!(!Arc::ptr_eq(first, &pending.cancel));
        }
        first = Some(pending.cancel.clone());
        let (hold, rx) = mpsc::channel();
        holds.push(hold);
        pending.rx = rx;
    }
}

fn interrupted_label_scenario(kind: &str) {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    let first = pending.cancel.clone();
    let (_hold, rx) = mpsc::channel();
    pending.rx = rx;
    let first_release_frame = frame;
    match kind {
        "hidden" => {
            app.set_folder_tree_pane_visible(false);
            navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
            app.set_folder_tree_pane_visible(true);
            // Reopening follows the active folder's actual drive. Finish its
            // ancestor listings before reading a new hit rect, while retaining
            // a fixed egui time so the gesture cannot simply expire.
            settle_navigation_tree_scans(&mut app, &ctx, &mut frame, 600.0);
            navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        }
        "focus" => {
            navigation_tree_frame(
                &mut app,
                &ctx,
                &mut frame,
                vec![egui::Event::WindowFocused(false)],
            );
            navigation_tree_frame(
                &mut app,
                &ctx,
                &mut frame,
                vec![egui::Event::WindowFocused(true)],
            );
        }
        "other-click" => {
            pointer_tree_click(&mut app, &ctx, &mut frame, egui::pos2(1500.0, 550.0));
        }
        "secondary" => {
            navigation_tree_frame(
                &mut app,
                &ctx,
                &mut frame,
                vec![
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Secondary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Secondary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        "touch-cancel" => {
            navigation_tree_frame(
                &mut app,
                &ctx,
                &mut frame,
                vec![egui::Event::Touch {
                    device_id: egui::TouchDeviceId(769),
                    id: egui::TouchId(1),
                    phase: egui::TouchPhase::Cancel,
                    pos,
                    force: None,
                }],
            );
        }
        "pane-focus" => {
            app.folder_pane.has_focus = false;
            navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        }
        "drive" => {
            let drive = app.folder_pane.selected_drive().unwrap().to_path_buf();
            app.folder_pane.select_drive(drive);
            app.folder_pane.test_settle();
            navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        }
        "reload" => {
            let active = app.current_folder.clone();
            let options =
                crate::folder_pane::FolderPaneListingOptions::from_settings(&app.settings);
            app.folder_pane
                .reload_for_active(active.as_deref(), options);
            app.folder_pane.test_settle();
            navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
        }
        _ => unreachable!(),
    }
    let pos = visible_label_click_rect(&ctx, &b).center();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    if kind == "hidden" {
        assert!(
            (frame - first_release_frame) as f64 / 60.0
                < ctx.options(|options| options.input_options.max_double_click_delay),
            "the hidden interval must cancel the gesture without relying on expiry"
        );
    }
    assert_label_expanded(&app, &b, false);
    assert!(
        first.load(Ordering::Relaxed),
        "{kind} must cancel the old gesture and permit a new single click"
    );
    assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, b);
}

#[test]
fn folder_pane_label_hidden_breaks_sequence() {
    interrupted_label_scenario("hidden");
}
#[test]
fn folder_pane_label_window_focus_loss_breaks_sequence() {
    interrupted_label_scenario("focus");
}
#[test]
fn folder_pane_label_other_click_breaks_sequence() {
    interrupted_label_scenario("other-click");
}
#[test]
fn folder_pane_label_secondary_click_breaks_sequence() {
    interrupted_label_scenario("secondary");
}
#[test]
fn folder_pane_label_touch_cancel_breaks_sequence() {
    interrupted_label_scenario("touch-cancel");
}
#[test]
fn folder_pane_label_pane_focus_loss_breaks_sequence() {
    interrupted_label_scenario("pane-focus");
}
#[test]
fn folder_pane_label_drive_reselection_breaks_sequence() {
    interrupted_label_scenario("drive");
}
#[test]
fn folder_pane_label_reload_breaks_sequence() {
    interrupted_label_scenario("reload");
}

fn label_overlay_frame(
    app: &mut App,
    ctx: &egui::Context,
    frame: &mut u64,
    events: Vec<egui::Event>,
    pos: egui::Pos2,
    kind: &str,
) {
    *frame += 1;
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
            app.input_update_begin(ctx);
            let keyboard_nav = app.handle_keyboard(ctx);
            let folder_pane_nav = app.render_folder_pane(ui).0;
            app.post_render_main_navigation(
                ctx,
                update_phases::MainFrameNavigation {
                    keyboard_nav,
                    folder_pane_nav,
                    ..empty_main_navigation()
                },
            );
            app.finish_folder_pane_presentation(ctx, true);
            match kind {
                "modal" => {
                    egui::Modal::new(egui::Id::unique("tree-test-foreign-modal")).show(ctx, |ui| {
                        ui.label("Other modal");
                    });
                }
                "popup" => {
                    egui::Popup::new(
                        egui::Id::unique("tree-test-popup"),
                        ctx.clone(),
                        pos,
                        egui::LayerId::background(),
                    )
                    .open(true)
                    .show(|ui| {
                        ui.set_min_size(egui::vec2(200.0, 100.0));
                        ui.label("Other popup");
                    });
                }
                "window" => {
                    egui::Window::new("Other window")
                        .fixed_pos(pos - egui::vec2(20.0, 40.0))
                        .fixed_size(egui::vec2(240.0, 140.0))
                        .fade_in(false)
                        .show(ctx, |ui| {
                            ui.label("Foreign input owner");
                        });
                }
                _ => unreachable!(),
            }
        },
    )
    .drop_without_applying_deltas();
}

fn label_overlay_scenario(kind: &str) {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    let first = pending.cancel.clone();
    let (_hold, rx) = mpsc::channel();
    pending.rx = rx;
    for _ in 0..2 {
        label_overlay_frame(&mut app, &ctx, &mut frame, vec![], pos, kind);
    }
    label_overlay_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![primary_event(pos, true)],
        pos,
        kind,
    );
    label_overlay_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![primary_event(pos, false)],
        pos,
        kind,
    );
    assert_label_expanded(&app, &b, false);
    assert!(
        !first.load(Ordering::Relaxed),
        "foreign input must not navigate the tree"
    );
    assert!(Arc::ptr_eq(
        &first,
        &app.folder_pane_open_pending.as_ref().unwrap().cancel
    ));
}

#[test]
fn folder_pane_label_foreign_modal_does_not_continue() {
    label_overlay_scenario("modal");
}
#[test]
fn folder_pane_label_popup_does_not_continue() {
    label_overlay_scenario("popup");
}
#[test]
fn folder_pane_label_floating_window_does_not_continue() {
    label_overlay_scenario("window");
}

fn full_label_rect(ctx: &egui::Context, path: &Path) -> egui::Rect {
    label_click_rect(ctx, path)
}

fn tree_label_at(app: &App, ctx: &egui::Context, pos: egui::Pos2) -> PathBuf {
    let viewport = ctx
        .data(|data| data.get_temp::<egui::Rect>(egui::Id::unique("folder_pane_test_viewport")))
        .unwrap();
    app.folder_pane
        .visible_rows()
        .into_iter()
        .find(|row| {
            full_label_rect(ctx, &row.path)
                .intersect(viewport)
                .contains(pos)
        })
        .expect("pointer must hit a visible label")
        .path
}

fn req40_scrolling_click_scenario(expanded: bool, clicks: u8) {
    let (mut app, ctx, mut frame, b, _) = navigation_tree_fixture();
    let root = b.parent().unwrap().to_path_buf();
    let mut children = vec![root.join("a"), root.join("b"), root.join("c")];
    let items: Vec<_> = (0..60).map(|i| root.join(format!("item-{i:03}"))).collect();
    for path in &items {
        std::fs::create_dir(path).unwrap();
        std::fs::create_dir(path.join("child")).unwrap();
        children.push(path.clone());
    }
    app.folder_pane = FolderPaneState::test_tree(&root, children);
    app.folder_pane.test_scan_subtree(root.clone());
    for path in &items {
        app.folder_pane
            .test_seed_listing(path, vec![path.join("child")], expanded);
    }
    app.folder_pane.set_cursor(root.clone());
    for _ in 0..3 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    }
    let viewport = ctx
        .data(|data| data.get_temp::<egui::Rect>(egui::Id::unique("folder_pane_test_viewport")))
        .unwrap();
    let x = items
        .iter()
        .filter(|path| {
            let rect = full_label_rect(&ctx, path);
            rect.top() > viewport.top()
                && rect.bottom() < viewport.bottom() - 10.0
                && rect.center().y > viewport.center().y + 60.0
        })
        .next_back()
        .expect("off-center X must be visible in a scrolling tree")
        .clone();
    let before = full_label_rect(&ctx, &x);
    let pos = before.center();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let pending = app
        .folder_pane_open_pending
        .as_mut()
        .expect("first click immediately opens X");
    assert_eq!(pending.path, x);
    let first = pending.cancel.clone();
    let (_held, rx) = mpsc::channel();
    let original_rx = std::mem::replace(&mut pending.rx, rx);
    // Keep the first click off-center while the navigation receive is held.
    for _ in 0..2 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    }
    let moved = full_label_rect(&ctx, &x);
    assert!(
        (before.center().y - moved.center().y).abs() < 0.1,
        "mouse navigation must preserve X's position: {before:?} -> {moved:?}"
    );
    for _ in 2..=clicks {
        let hit = tree_label_at(&app, &ctx, pos);
        assert_eq!(hit, x, "the stationary click must still hit its first row");
        pointer_tree_click(&mut app, &ctx, &mut frame, pos);
        assert!(
            !first.load(Ordering::Relaxed),
            "repeat must not cancel X's scan (hit {hit:?})"
        );
        let pending = app.folder_pane_open_pending.as_ref().unwrap();
        assert!(Arc::ptr_eq(&first, &pending.cancel));
        assert_eq!(pending.path, x);
        let rows = app.folder_pane.visible_rows();
        assert_eq!(
            rows.iter().find(|row| row.path == x).unwrap().expanded,
            !expanded
        );
        assert!(rows.iter().any(|row| row.path == x && row.is_cursor));
        // Rebuild the row hit table after opening/closing X.
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    }
    app.folder_pane_open_pending.as_mut().unwrap().rx = original_rx;
    finish_label_navigation(&mut app, &ctx, &x);
    assert!(!first.load(Ordering::Relaxed));
    assert_eq!(
        app.folder_pane
            .visible_rows()
            .iter()
            .find(|row| row.path == x)
            .unwrap()
            .expanded,
        !expanded
    );
}

#[test]
fn folder_pane_label_req40_scrolled_closed_double() {
    req40_scrolling_click_scenario(false, 2);
}
#[test]
fn folder_pane_label_req40_scrolled_closed_triple() {
    req40_scrolling_click_scenario(false, 3);
}
#[test]
fn folder_pane_label_req40_scrolled_open_double() {
    req40_scrolling_click_scenario(true, 2);
}
#[test]
fn folder_pane_label_req40_scrolled_open_triple() {
    req40_scrolling_click_scenario(true, 3);
}

#[test]
fn folder_pane_label_req40_completed_navigation_collapses_prior_ancestors() {
    let (mut app, ctx, mut frame, b, _) = navigation_tree_fixture();
    let root = b.parent().unwrap().to_path_buf();
    let prior = root.join("m-prior");
    for name in ["leaf-0", "leaf-1"] {
        std::fs::create_dir_all(prior.join(name)).unwrap();
    }
    let x = root.join("x-target");
    std::fs::create_dir_all(x.join("child")).unwrap();
    for i in 0..8 {
        std::fs::create_dir(root.join(format!("zz-tail-{i:02}"))).unwrap();
    }
    app.folder_pane = FolderPaneState::default();
    app.folder_pane.test_scan_subtree(root);
    app.load_folder(prior.join("leaf-1"));
    settle(&mut app, &ctx);
    const HEIGHT: f32 = 1400.0; // All rows fit; ancestor removal must move X, not restore an offset.
    // Default state is suspended. Only a real drawing boundary resumes it and
    // starts the ancestor scans; sync-only/has_pending=false is not completion.
    settle_navigation_tree_scans(&mut app, &ctx, &mut frame, HEIGHT);
    app.folder_pane.set_cursor(x.clone());
    for _ in 0..3 {
        navigation_tree_frame_height(&mut app, &ctx, &mut frame, vec![], HEIGHT);
    }
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .find(|row| row.path == prior)
            .unwrap()
            .expanded
    );
    let before = full_label_rect(&ctx, &x);
    let pos = before.center();
    let click = |app: &mut App, frame: &mut u64| {
        let event = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        navigation_tree_frame_height(
            app,
            &ctx,
            frame,
            vec![egui::Event::PointerMoved(pos), event(true)],
            HEIGHT,
        );
        navigation_tree_frame_height(app, &ctx, frame, vec![event(false)], HEIGHT);
    };
    click(&mut app, &mut frame);
    let first = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    finish_label_navigation(&mut app, &ctx, &x);
    for _ in 0..2 {
        navigation_tree_frame_height(&mut app, &ctx, &mut frame, vec![], HEIGHT);
    }
    assert!(
        !app.folder_pane
            .visible_rows()
            .iter()
            .find(|row| row.path == prior)
            .unwrap()
            .expanded
    );
    let after = full_label_rect(&ctx, &x);
    assert!(
        after.top() < before.top() - 30.0,
        "ancestor removal must shift X upward: {before:?} -> {after:?}"
    );
    let hit = tree_label_at(&app, &ctx, pos);
    assert_ne!(hit, x);
    let hit_expanded = app
        .folder_pane
        .visible_rows()
        .iter()
        .find(|row| row.path == hit)
        .unwrap()
        .expanded;
    click(&mut app, &mut frame);
    assert!(!first.load(Ordering::Relaxed));
    assert!(
        app.folder_pane_open_pending.is_none(),
        "must not start navigation to {hit:?}"
    );
    assert_eq!(app.current_folder.as_ref(), Some(&x));
    let rows = app.folder_pane.visible_rows();
    assert!(
        rows.iter()
            .any(|row| row.path == x && row.expanded && row.is_cursor)
    );
    assert_eq!(
        rows.iter().find(|row| row.path == hit).unwrap().expanded,
        hit_expanded
    );
}

#[test]
fn folder_pane_label_r1_long_sequence_preserves_the_first_pending_open() {
    let (mut app, ctx, mut frame, b, _) = navigation_tree_fixture();
    let child = b.join("child");
    std::fs::create_dir(&child).unwrap();
    app.folder_pane
        .test_seed_listing(&b, vec![child.clone()], false);
    for _ in 0..3 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    }
    let pos = visible_label_click_rect(&ctx, &b).center();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let first_frame = frame;
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    assert_eq!(pending.path, b);
    let first = pending.cancel.clone();
    let (_held_reply, rx) = mpsc::channel();
    let original_rx = std::mem::replace(&mut pending.rx, rx);
    for _ in 0..4 {
        frame += 13; // press + release add 2 frames: releases are 0.25s apart.
        pointer_tree_click(&mut app, &ctx, &mut frame, pos);
        assert!(
            !first.load(Ordering::Relaxed),
            "long repeat must not cancel the first scan at frame {frame}"
        );
        let pending = app.folder_pane_open_pending.as_ref().unwrap();
        assert!(
            Arc::ptr_eq(&first, &pending.cancel),
            "long repeat must not replace pending"
        );
        assert_eq!(pending.path, b);
        assert!(
            app.folder_pane
                .visible_rows()
                .iter()
                .find(|row| row.path == b)
                .unwrap()
                .expanded,
            "only the second click toggles the branch"
        );
    }
    assert!(
        (frame - first_frame) as f64 / 60.0
            > ctx.options(|options| options.input_options.max_double_click_delay * 2.0)
    );
    app.folder_pane_open_pending.as_mut().unwrap().rx = original_rx;
    finish_label_navigation(&mut app, &ctx, &b);
    settle_tree_scans(&mut app, &ctx, &mut frame);
    assert!(!first.load(Ordering::Relaxed));
    let rows = app.folder_pane.visible_rows();
    assert!(rows.iter().find(|row| row.path == b).unwrap().expanded);
    assert!(rows.iter().any(|row| row.path == child));
}

#[test]
fn folder_pane_label_boundary_nearby_row_opens_first_row_once() {
    let (mut app, ctx, mut frame, b, c) = navigation_tree_fixture();
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    let rb = visible_label_click_rect(&ctx, &b);
    let rc = visible_label_click_rect(&ctx, &c);
    let first_pos = egui::pos2(rb.center().x, rb.bottom() - 0.1);
    let second_pos = egui::pos2(rb.center().x, rc.top() + 0.1);
    assert!(first_pos.distance(second_pos) < 6.0);
    pointer_tree_click(&mut app, &ctx, &mut frame, first_pos);
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    let first = pending.cancel.clone();
    let (_hold, rx) = mpsc::channel();
    pending.rx = rx;
    pointer_tree_click(&mut app, &ctx, &mut frame, second_pos);
    assert!(!first.load(Ordering::Relaxed));
    assert!(Arc::ptr_eq(
        &first,
        &app.folder_pane_open_pending.as_ref().unwrap().cancel
    ));
    assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, b);
    assert_label_expanded(&app, &b, true);
    assert_label_expanded(&app, &c, false);
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == b && row.is_cursor)
    );
}

// Poll the real navigation/adoption path without advancing egui time. Real scan
// duration must not decide whether the next synthetic click is a double click.
fn finish_label_navigation(app: &mut App, ctx: &egui::Context, expected: &Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.folder_pane_open_pending.is_some() {
        app.post_render_main_navigation(ctx, empty_main_navigation());
        settle(app, ctx);
        assert!(
            std::time::Instant::now() < deadline,
            "label navigation did not finish"
        );
        std::thread::yield_now();
    }
    settle(app, ctx);
    app.sync_folder_pane_state(ctx);
    assert_eq!(app.current_folder.as_deref(), Some(expected));
}

fn label_double_click_scenario(scenario: &str) {
    let (mut app, ctx, mut frame, b, _) = navigation_tree_fixture();
    let child = b.join("child");
    let empty = scenario == "empty";
    if !empty {
        std::fs::create_dir(&child).unwrap();
    }
    let ancestor = scenario.starts_with("ancestor");
    if ancestor {
        // Automatic ancestor reveal requires the actual drive root. The
        // arbitration fixture intentionally selects a subtree as its root,
        // which suppresses that reveal. Keep host enumeration isolated via
        // the existing synthetic-ancestor/real-subtree backend instead.
        app.folder_pane = FolderPaneState::default();
        app.folder_pane
            .test_scan_subtree(b.parent().unwrap().to_path_buf());
    }
    if ancestor || scenario == "active" {
        app.load_folder(if ancestor { child.clone() } else { b.clone() });
        settle(&mut app, &ctx);
        app.sync_folder_pane_state(&ctx);
    }
    app.folder_pane.test_seed_listing(
        &b,
        if empty { vec![] } else { vec![child.clone()] },
        scenario == "expanded",
    );
    settle_tree_scans(&mut app, &ctx, &mut frame);
    // Finish the cursor reveal at the same viewport size used by the pointer
    // frames. The smaller tree_frame setup must not move the hit target when
    // the first click publishes its normal cursor reveal.
    app.folder_pane.set_cursor(b.clone());
    for _ in 0..3 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    }
    let initial = app
        .folder_pane
        .visible_rows()
        .into_iter()
        .find(|row| row.path == b)
        .unwrap();
    assert_eq!(initial.expanded, ancestor || scenario == "expanded");
    assert!(
        initial.has_children_or_unknown,
        "empty branches retain the arrow too"
    );
    let pos = visible_label_click_rect(&ctx, &b).center();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    assert!(app.folder_pane.has_focus);
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == b && row.is_cursor)
    );
    if scenario == "active" {
        assert!(app.folder_pane_open_pending.is_none());
        pointer_tree_click(&mut app, &ctx, &mut frame, pos);
        assert!(app.folder_pane_open_pending.is_none());
    } else {
        let pending = app
            .folder_pane_open_pending
            .as_mut()
            .expect("first click starts navigation immediately");
        assert_eq!(pending.path, b);
        let first = pending.cancel.clone();
        let (_held_reply, held_rx) = mpsc::channel();
        let original_rx = std::mem::replace(&mut pending.rx, held_rx);
        if scenario == "ancestor-before" {
            app.folder_pane_open_pending.as_mut().unwrap().rx = original_rx;
            finish_label_navigation(&mut app, &ctx, &b);
            // sync has already closed the auto-expanded ancestor. The second
            // click must keep that target instead of reopening it.
            assert!(
                !app.folder_pane
                    .visible_rows()
                    .iter()
                    .find(|row| row.path == b)
                    .unwrap()
                    .expanded
            );
            pointer_tree_click(&mut app, &ctx, &mut frame, pos);
            assert!(
                app.folder_pane_open_pending.is_none(),
                "completed navigation must not restart"
            );
        } else {
            pointer_tree_click(&mut app, &ctx, &mut frame, pos);
            if scenario == "triple" {
                pointer_tree_click(&mut app, &ctx, &mut frame, pos);
            }
            assert!(
                !first.load(Ordering::Relaxed),
                "repeat must not cancel the first scan"
            );
            let pending = app
                .folder_pane_open_pending
                .as_mut()
                .expect("first scan is still held");
            assert!(
                Arc::ptr_eq(&first, &pending.cancel),
                "repeat must not replace pending"
            );
            assert_eq!(pending.path, b);
            pending.rx = original_rx;
            finish_label_navigation(&mut app, &ctx, &b);
        }
        assert!(!first.load(Ordering::Relaxed));
    }
    settle_tree_scans(&mut app, &ctx, &mut frame);
    let rows = app.folder_pane.visible_rows();
    let row = rows.iter().find(|row| row.path == b).unwrap();
    assert_eq!(
        row.expanded, !initial.expanded,
        "{scenario}: one toggle from the first display"
    );
    assert_eq!(
        rows.iter().any(|row| row.path == child),
        !initial.expanded && !empty
    );
    assert_eq!(app.current_folder.as_ref(), Some(&b));
}

#[test]
fn folder_pane_label_double_click_closed_branch() {
    label_double_click_scenario("closed");
}
#[test]
fn folder_pane_label_double_click_user_expanded_branch() {
    label_double_click_scenario("expanded");
}
#[test]
fn folder_pane_label_double_click_ancestor_navigation_before_second() {
    label_double_click_scenario("ancestor-before");
}
#[test]
fn folder_pane_label_double_click_ancestor_navigation_after_second() {
    label_double_click_scenario("ancestor-after");
}
#[test]
fn folder_pane_label_double_click_active_has_no_navigation() {
    label_double_click_scenario("active");
}
#[test]
fn folder_pane_label_double_click_confirmed_empty_branch() {
    label_double_click_scenario("empty");
}
#[test]
fn folder_pane_label_triple_click_toggles_once() {
    label_double_click_scenario("triple");
}

#[test]
fn folder_pane_label_clicks_beyond_double_delay_navigate_twice() {
    let (mut app, ctx, mut frame, b, _) = navigation_tree_fixture();
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    let pos = visible_label_click_rect(&ctx, &b).center();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    let first = pending.cancel.clone();
    let (_held_reply, rx) = mpsc::channel();
    pending.rx = rx;
    frame += 20; // More than egui's 0.3-second double-click delay.
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    assert!(
        first.load(Ordering::Relaxed),
        "a new single click replaces the pending navigation"
    );
    let pending = app.folder_pane_open_pending.as_ref().unwrap();
    assert!(!Arc::ptr_eq(&first, &pending.cancel));
    assert!(
        !app.folder_pane
            .visible_rows()
            .iter()
            .find(|row| row.path == b)
            .unwrap()
            .expanded
    );
    finish_label_navigation(&mut app, &ctx, &b);
}

#[test]
fn folder_pane_label_distant_row_navigates_to_second_row() {
    let (mut app, ctx, mut frame, b, c) = navigation_tree_fixture();
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    let first_pos = visible_label_click_rect(&ctx, &b).center();
    let second_pos = visible_label_click_rect(&ctx, &c).center();
    assert!(first_pos.distance(second_pos) > 6.0);
    pointer_tree_click(&mut app, &ctx, &mut frame, first_pos);
    let first = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    let (_hold, rx) = mpsc::channel();
    app.folder_pane_open_pending.as_mut().unwrap().rx = rx;
    pointer_tree_click(&mut app, &ctx, &mut frame, second_pos);
    assert!(first.load(Ordering::Relaxed));
    assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, c);
    assert_label_expanded(&app, &b, false);
    assert_label_expanded(&app, &c, false);
}

#[test]
fn folder_pane_label_arrow_and_keyboard_keep_existing_behavior() {
    let (mut app, ctx, mut frame, b, _) = navigation_tree_fixture();
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    let label = visible_label_click_rect(&ctx, &b);
    // The arrow and label allocations are 2pt apart. These points cross the
    // arrow/label boundary within egui's double-click distance.
    let arrow_pos = egui::pos2(label.left() - 3.0, label.center().y);
    let label_pos = egui::pos2(label.left() + 0.5, label.center().y);
    pointer_tree_click(&mut app, &ctx, &mut frame, label_pos);
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    let first = pending.cancel.clone();
    let (_held_reply, rx) = mpsc::channel();
    pending.rx = rx;
    pointer_tree_click(&mut app, &ctx, &mut frame, arrow_pos);
    assert!(!first.load(Ordering::Relaxed), "arrow only expands");
    assert!(Arc::ptr_eq(
        &first,
        &app.folder_pane_open_pending.as_ref().unwrap().cancel
    ));
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .find(|row| row.path == b)
            .unwrap()
            .expanded
    );
    pointer_tree_click(&mut app, &ctx, &mut frame, label_pos);
    assert!(
        first.load(Ordering::Relaxed),
        "arrow clears the old label sequence"
    );
    assert!(!Arc::ptr_eq(
        &first,
        &app.folder_pane_open_pending.as_ref().unwrap().cancel
    ));
    assert_eq!(
        app.folder_pane_open_pending.as_ref().unwrap().path,
        b,
        "arrow must clear the label record"
    );
    finish_label_navigation(&mut app, &ctx, &b);
    frame += 20;
    pointer_tree_click(&mut app, &ctx, &mut frame, arrow_pos);
    assert!(
        !app.folder_pane
            .visible_rows()
            .iter()
            .find(|row| row.path == b)
            .unwrap()
            .expanded
    );
    assert!(app.folder_pane_open_pending.is_none());
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![tree_key_event(egui::Key::ArrowRight)],
    );
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .find(|row| row.path == b)
            .unwrap()
            .expanded
    );
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![tree_key_event(egui::Key::ArrowLeft)],
    );
    assert!(
        !app.folder_pane
            .visible_rows()
            .iter()
            .find(|row| row.path == b)
            .unwrap()
            .expanded
    );
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![tree_key_event(egui::Key::ArrowDown)],
    );
    let target = app
        .folder_pane
        .visible_rows()
        .into_iter()
        .find(|row| row.is_cursor)
        .unwrap()
        .path;
    assert_ne!(target, b);
    assert!(
        app.folder_pane_open_pending.is_none(),
        "arrows move only the cursor"
    );
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![tree_key_event(egui::Key::ArrowUp)],
    );
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![tree_key_event(egui::Key::Enter)],
    );
    assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, b);
    finish_label_navigation(&mut app, &ctx, &b);
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
    let rect = label_click_rect(&ctx, &b);
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
    prepare_headless_folder_pane_fixture(&mut app);
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
            prepare_headless_folder_pane_fixture(&mut app);
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
        prepare_headless_folder_pane_fixture(&mut app);
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
    prepare_headless_folder_pane_fixture(&mut app);
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
    prepare_headless_folder_pane_fixture(&mut app);
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

// Match actual pane drawing/adoption order while keeping wall-clock worker
// latency out of the synthetic double-click interval.
fn settle_navigation_tree_scans(app: &mut App, ctx: &egui::Context, frame: &mut u64, height: f32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let before = *frame;
    loop {
        let mut fixed_frame = before;
        navigation_tree_frame_height(app, ctx, &mut fixed_frame, vec![], height);
        if !app.folder_pane.has_pending() {
            *frame = fixed_frame;
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "navigation tree scans did not finish"
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
    prepare_headless_folder_pane_fixture(&mut app);
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
    prepare_headless_folder_pane_fixture(&mut app);
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
        prepare_headless_folder_pane_fixture(&mut app);
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
    prepare_headless_folder_pane_fixture(&mut app);
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
        prepare_headless_folder_pane_fixture(&mut app);
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

#[test]
fn folder_pane_label_ordered_arrow_then_nearby_label_is_single() {
    let (mut app, ctx, mut frame, b, _) = navigation_tree_fixture();
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    let primary_event = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
    let label = visible_label_click_rect(&ctx, &b);
    let arrow = egui::pos2(label.left() - 3.9, label.center().y);
    let pos = egui::pos2(label.left() + 0.1, label.center().y);
    assert!(arrow.distance(pos) < 6.0);
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            egui::Event::PointerMoved(arrow),
            primary_event(arrow, true),
            primary_event(arrow, false),
            egui::Event::PointerMoved(pos),
            primary_event(pos, true),
            primary_event(pos, false),
        ],
    );
    let label_id = ctx
        .data(|data| data.get_temp::<egui::Id>(egui::Id::unique(("folder_pane_test_label_id", &b))))
        .unwrap();
    let witnesses = ctx.pointer_button_targets();
    assert_eq!(
        witnesses.len(),
        4,
        "each button edge has its own hit witness"
    );
    let arrow_id = witnesses[0].3.expect("arrow press must hit a widget");
    assert_ne!(arrow_id, label_id, "arrow and label are distinct widgets");
    assert!(ctx.read_response(arrow_id).unwrap().rect.contains(arrow));
    for (index, expected) in [arrow_id, arrow_id, label_id, label_id]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            witnesses[index].3,
            Some(expected),
            "edge {index} has its own correct capture"
        );
        assert!(witnesses[index].4, "edge {index} is eligible");
    }
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == b && row.expanded),
        "arrow click must expand its row even before a nearby label click in the same pass"
    );
    assert_eq!(
        app.folder_pane_open_pending
            .as_ref()
            .expect("label remains a single click")
            .path,
        b
    );
}

#[test]
fn folder_pane_label_ordered_arrow_then_name_double_applies_toggle_last() {
    let (mut app, ctx, mut frame, b, _, _) = ordered_label_fixture();
    let rect = visible_label_click_rect(&ctx, &b);
    let arrow = egui::pos2(rect.left() - 3.9, rect.center().y);
    let label = egui::pos2(rect.left() + 0.1, rect.center().y);
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            egui::Event::PointerMoved(arrow),
            primary_event(arrow, true),
            primary_event(arrow, false),
            egui::Event::PointerMoved(label),
            primary_event(label, true),
            primary_event(label, false),
            primary_event(label, true),
            primary_event(label, false),
        ],
    );
    // The name was first clicked while its last presented row was closed.
    // Apply Arrow first, then the name gesture's explicit target (open).
    assert_label_expanded(&app, &b, true);
    assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, b);
}

fn accesskit_label_click(ctx: &egui::Context, path: &Path) -> egui::Event {
    let id = ctx
        .data(|data| {
            data.get_temp::<egui::Id>(egui::Id::unique(("folder_pane_test_label_id", path)))
        })
        .unwrap();
    egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
        action: egui::accesskit::Action::Click,
        target_tree: egui::accesskit::TreeId::ROOT,
        target_node: id.accesskit_id(),
        data: None,
    })
}

#[test]
fn folder_pane_label_pointer_arrow_preserves_other_row_accesskit_activation() {
    let (mut app, ctx, mut frame, b, c, _) = ordered_label_fixture();
    let root = b.parent().unwrap().to_path_buf();
    let child = b.join("child");
    app.folder_pane = FolderPaneState::test_tree(&root, vec![b.clone(), c.clone()]);
    app.folder_pane.test_scan_subtree(root.clone());
    app.folder_pane
        .test_seed_listing(&b, vec![child.clone()], false);
    app.folder_pane.set_cursor(b.clone());
    ctx.enable_accesskit();
    ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
    for _ in 0..3 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    }
    let rect = visible_label_click_rect(&ctx, &b);
    let arrow = egui::pos2(rect.left() - 3.9, rect.center().y);
    // Warm A's real displayed child cache, then return to [root, A, B].
    pointer_tree_click(&mut app, &ctx, &mut frame, arrow);
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == child)
    );
    pointer_tree_click(&mut app, &ctx, &mut frame, arrow);
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    assert_eq!(
        app.folder_pane
            .visible_rows()
            .into_iter()
            .map(|row| row.path)
            .collect::<Vec<_>>(),
        vec![root, b.clone(), c.clone()]
    );
    let c_before = full_label_rect(&ctx, &c);
    let c_id = ctx
        .data(|data| {
            data.get_temp::<egui::Id>(egui::Id::unique(("folder_pane_test_label_id", c.as_path())))
        })
        .unwrap();
    let rect = visible_label_click_rect(&ctx, &b);
    let arrow = egui::pos2(rect.left() - 3.9, rect.center().y);
    let access = accesskit_label_click(&ctx, &c);
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            egui::Event::PointerMoved(arrow),
            primary_event(arrow, true),
            primary_event(arrow, false),
            access,
        ],
    );
    assert_label_expanded(&app, &b, true);
    assert!(
        app.folder_pane
            .visible_rows()
            .iter()
            .any(|row| row.path == child)
    );
    let c_after_id = ctx
        .data(|data| {
            data.get_temp::<egui::Id>(egui::Id::unique(("folder_pane_test_label_id", c.as_path())))
        })
        .unwrap();
    assert_eq!(
        c_after_id, c_id,
        "inserting A's child must preserve B's widget identity in this same pass"
    );
    assert!(
        full_label_rect(&ctx, &c).top() > c_before.top() + 20.0,
        "B must actually move down when A's cached child is inserted"
    );
    assert_eq!(
        app.folder_pane_open_pending
            .as_ref()
            .expect("another widget's non-pointer activation must survive")
            .path,
        c
    );
}

#[test]
fn folder_pane_label_same_widget_pointer_and_accesskit_click_are_applied_once() {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    ctx.enable_accesskit();
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    let access = accesskit_label_click(&ctx, &b);
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            egui::Event::PointerMoved(pos),
            primary_event(pos, true),
            primary_event(pos, false),
            access,
        ],
    );
    assert_label_expanded(&app, &b, false);
    let first = app
        .folder_pane_open_pending
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    assert_label_expanded(&app, &b, true);
    assert!(!first.load(Ordering::Relaxed));
}

#[test]
fn folder_pane_label_touch_arrow_taps_toggle_each_time_without_navigation() {
    let (mut app, ctx, mut frame, b, _, _) = ordered_label_fixture();
    let rect = visible_label_click_rect(&ctx, &b);
    let pos = egui::pos2(rect.left() - 3.9, rect.center().y);
    let touch = |phase| egui::Event::Touch {
        device_id: egui::TouchDeviceId(769),
        id: egui::TouchId(1),
        phase,
        pos,
        force: None,
    };
    for expected in [true, false] {
        navigation_tree_frame(
            &mut app,
            &ctx,
            &mut frame,
            vec![
                touch(egui::TouchPhase::Start),
                egui::Event::PointerMoved(pos),
                primary_event(pos, true),
            ],
        );
        navigation_tree_frame(
            &mut app,
            &ctx,
            &mut frame,
            vec![
                touch(egui::TouchPhase::End),
                primary_event(pos, false),
                egui::Event::PointerGone,
            ],
        );
        assert_label_expanded(&app, &b, expected);
        assert!(app.folder_pane_open_pending.is_none());
    }
}

#[test]
fn folder_pane_label_discarded_root_pass_does_not_replay_single_click() {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, true)],
    );
    frame += 1;
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 600.0),
            )),
            time: Some(frame as f64 / 60.0),
            events: vec![primary_event(pos, false)],
            ..Default::default()
        },
        |_ui| {
            app.input_update_begin(&ctx);
            // A root presentation switch can skip the pane application point.
            app.finish_folder_pane_presentation(&ctx, false);
        },
    )
    .drop_without_applying_deltas();
    assert!(app.folder_pane_open_pending.is_none());
    for _ in 0..3 {
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    }
    assert!(
        app.folder_pane_open_pending.is_none(),
        "a discarded pass cannot navigate on return"
    );
    assert_label_expanded(&app, &b, false);
}

#[test]
fn folder_pane_label_second_press_changes_display_only_at_application_point() {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    let first = pending.cancel.clone();
    let (_hold, rx) = mpsc::channel();
    pending.rx = rx;
    let focus = app.folder_pane.has_focus;
    let reveal = app.folder_pane.cursor_scroll;
    frame += 1;
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 600.0),
            )),
            time: Some(frame as f64 / 60.0),
            events: vec![primary_event(pos, true)],
            ..Default::default()
        },
        |ui| {
            app.input_update_begin(&ctx);
            assert_label_expanded(&app, &b, false);
            assert_eq!(app.folder_pane.has_focus, focus);
            assert_eq!(app.folder_pane.cursor_scroll, reveal);
            let keyboard_nav = app.handle_keyboard(&ctx);
            let folder_pane_nav = app.render_folder_pane(ui).0;
            app.post_render_main_navigation(
                &ctx,
                update_phases::MainFrameNavigation {
                    keyboard_nav,
                    folder_pane_nav,
                    ..empty_main_navigation()
                },
            );
            app.finish_folder_pane_presentation(&ctx, true);
        },
    )
    .drop_without_applying_deltas();
    assert_label_expanded(&app, &b, true);
    assert!(!first.load(Ordering::Relaxed));
    assert!(Arc::ptr_eq(
        &first,
        &app.folder_pane_open_pending.as_ref().unwrap().cancel
    ));
    assert_eq!(app.folder_pane.has_focus, focus);
    assert_eq!(app.folder_pane.cursor_scroll, reveal);
}

#[test]
fn folder_pane_label_more_than_six_points_on_same_row_starts_new_navigation() {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    let first = pending.cancel.clone();
    let (_hold, rx) = mpsc::channel();
    pending.rx = rx;
    let second = pos + egui::vec2(7.0, 0.0);
    assert!(visible_label_click_rect(&ctx, &b).contains(second));
    pointer_tree_click(&mut app, &ctx, &mut frame, second);
    assert!(first.load(Ordering::Relaxed));
    assert_label_expanded(&app, &b, false);
    assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, b);
}

#[test]
fn folder_pane_label_ordered_pointer_arrow_then_other_label_deduplicates_arrow_accesskit() {
    let (mut app, ctx, mut frame, a, b, _) = ordered_label_fixture();
    ctx.enable_accesskit();
    ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    let rect = visible_label_click_rect(&ctx, &a);
    let arrow = egui::pos2(rect.left() - 3.9, rect.center().y);
    // Capture the real arrow widget id without clicking it or changing state.
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(arrow), primary_event(arrow, true)],
    );
    let arrow_id = ctx.pointer_button_targets()[0]
        .3
        .expect("the real arrow press has a witness");
    let outside = egui::pos2(1500.0, 550.0);
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            egui::Event::PointerMoved(outside),
            primary_event(outside, false),
        ],
    );
    assert_label_expanded(&app, &a, false);
    assert!(app.folder_pane_open_pending.is_none());
    frame += 20;
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    let label = visible_label_click_rect(&ctx, &b).center();
    let label_id = ctx
        .data(|data| {
            data.get_temp::<egui::Id>(egui::Id::unique(("folder_pane_test_label_id", b.as_path())))
        })
        .unwrap();
    let access = egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
        action: egui::accesskit::Action::Click,
        target_tree: egui::accesskit::TreeId::ROOT,
        target_node: arrow_id.accesskit_id(),
        data: None,
    });
    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![
            egui::Event::PointerMoved(arrow),
            primary_event(arrow, true),
            primary_event(arrow, false),
            egui::Event::PointerMoved(label),
            primary_event(label, true),
            primary_event(label, false),
            access,
        ],
    );
    let witnesses = ctx.pointer_button_targets();
    assert_eq!(witnesses.len(), 4);
    for (index, expected) in [arrow_id, arrow_id, label_id, label_id]
        .into_iter()
        .enumerate()
    {
        assert_eq!(witnesses[index].3, Some(expected));
        assert!(witnesses[index].4);
    }
    assert_label_expanded(&app, &a, true);
    assert_label_expanded(&app, &b, false);
    assert_eq!(
        app.folder_pane_open_pending
            .as_ref()
            .expect("the later pointer label remains a single click")
            .path,
        b
    );
}

fn late_modal_rejects_tree_result(kind: &str) {
    let (mut app, ctx, mut frame, b, _, label) = ordered_label_fixture();
    let target = if kind == "arrow" {
        egui::pos2(visible_label_click_rect(&ctx, &b).left() - 3.9, label.y)
    } else {
        label
    };
    let mut hold = None;
    let first = if kind == "toggle" {
        pointer_tree_click(&mut app, &ctx, &mut frame, label);
        let pending = app.folder_pane_open_pending.as_mut().unwrap();
        let token = pending.cancel.clone();
        let (tx, rx) = mpsc::channel();
        hold = Some(tx);
        pending.rx = rx;
        Some(token)
    } else {
        navigation_tree_frame(
            &mut app,
            &ctx,
            &mut frame,
            vec![
                egui::Event::PointerMoved(target),
                primary_event(target, true),
            ],
        );
        None
    };
    let cursor = app
        .folder_pane
        .visible_rows()
        .into_iter()
        .find(|row| row.is_cursor)
        .map(|row| row.path);
    let focus = app.folder_pane.has_focus;
    let reveal = app.folder_pane.cursor_scroll;
    let generation = app.items_generation;
    frame += 1;
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 600.0),
            )),
            time: Some(frame as f64 / 60.0),
            events: vec![primary_event(target, kind == "toggle")],
            ..Default::default()
        },
        |ui| {
            app.input_update_begin(&ctx);
            assert_label_expanded(&app, &b, false);
            // This is the actual boundary: a later shortcut/worker can start a
            // modal after recognition but before the pane applies its results.
            app.show_preferences = true;
            let folder_pane_nav = app.render_folder_pane(ui).0;
            app.post_render_main_navigation(
                &ctx,
                update_phases::MainFrameNavigation {
                    folder_pane_nav,
                    ..empty_main_navigation()
                },
            );
            app.finish_folder_pane_presentation(&ctx, true);
        },
    )
    .drop_without_applying_deltas();
    for phase in 0..3 {
        assert_label_expanded(&app, &b, false);
        assert_eq!(app.folder_pane.has_focus, focus, "{kind} phase {phase}");
        assert_eq!(
            app.folder_pane
                .visible_rows()
                .into_iter()
                .find(|row| row.is_cursor)
                .map(|row| row.path),
            cursor
        );
        assert_eq!(app.folder_pane.cursor_scroll, reveal);
        assert_eq!(app.items_generation, generation);
        if let Some(first) = first.as_ref() {
            assert!(!first.load(Ordering::Relaxed));
            assert!(Arc::ptr_eq(
                first,
                &app.folder_pane_open_pending.as_ref().unwrap().cancel
            ));
        } else {
            assert!(app.folder_pane_open_pending.is_none());
        }
        app.show_preferences = false;
        navigation_tree_frame(&mut app, &ctx, &mut frame, vec![]);
    }
    drop(hold);
}

#[test]
fn folder_pane_label_late_modal_discards_single_without_replay() {
    late_modal_rejects_tree_result("single");
}
#[test]
fn folder_pane_label_late_modal_discards_arrow_without_replay() {
    late_modal_rejects_tree_result("arrow");
}
#[test]
fn folder_pane_label_late_modal_discards_toggle_without_replay() {
    late_modal_rejects_tree_result("toggle");
}

#[test]
fn folder_pane_label_snapshot_fixed_pane_rejects_continuation() {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    let (_hold, rx) = mpsc::channel();
    pending.rx = rx;
    let image = app.tmp.path().join("snapshot-source.jpg");
    std::fs::write(&image, b"fixture").unwrap();
    app.items = vec![crate::grid_item::GridItem::Image(image)];
    app.thumbnails = vec![crate::grid_item::ThumbnailState::Pending];
    app.visible_indices = vec![0];
    app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    app.settle_snapshot_page_index_for_test();
    assert!(app.is_snapshot_active());
    assert!(app.folder_pane_disabled());
    let folder = app.current_folder.clone();
    let pending = app
        .folder_pane_open_pending
        .as_ref()
        .map(|pending| pending.cancel.clone());
    let cursor = app
        .folder_pane
        .visible_rows()
        .into_iter()
        .find(|row| row.is_cursor)
        .map(|row| row.path);
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    assert_label_expanded(&app, &b, false);
    assert_eq!(app.current_folder, folder);
    assert_eq!(
        app.folder_pane
            .visible_rows()
            .into_iter()
            .find(|row| row.is_cursor)
            .map(|row| row.path),
        cursor
    );
    match pending {
        Some(pending) => assert!(Arc::ptr_eq(
            &pending,
            &app.folder_pane_open_pending
                .as_ref()
                .expect("the disabled click must not replace the existing scan")
                .cancel
        )),
        None => assert!(app.folder_pane_open_pending.is_none()),
    }
}

#[test]
fn folder_pane_label_child_input_preserves_root_gesture_display_and_results() {
    let (mut app, ctx, mut frame, b, _, pos) = ordered_label_fixture();
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    assert_label_expanded(&app, &b, false);
    let pending = app.folder_pane_open_pending.as_mut().unwrap();
    let first = pending.cancel.clone();
    let (_hold, rx) = mpsc::channel();
    pending.rx = rx;

    let display_snapshot = |app: &App| {
        let display = app
            .folder_pane
            .label_display
            .as_ref()
            .expect("root painted its tree");
        (
            display.pass,
            display.rect,
            display
                .rows
                .iter()
                .map(|row| {
                    (
                        row.path.clone(),
                        row.label,
                        row.label_rect,
                        row.arrow,
                        row.arrow_rect,
                    )
                })
                .collect::<Vec<_>>(),
            display.layer,
        )
    };
    let cache = display_snapshot(&app);
    let root_pass = cache.0;
    let root_completed = ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT);
    assert_eq!(root_pass.checked_add(1), Some(root_completed));
    assert!(
        !app.folder_pane.begin_label_input(root_pass),
        "the root input owner already belongs to this pass"
    );
    assert!(app.folder_pane.take_label_results(root_pass).is_empty());
    assert!(app.folder_pane.label_gesture_requires_tree_focus());
    for row in &cache.2 {
        assert!(!app.folder_pane.label_pointer_activation_owned(row.1));
        assert!(!app.folder_pane.label_pointer_activation_owned(row.3));
    }
    let rows = app.folder_pane.visible_rows();
    let focus = app.folder_pane.has_focus;
    let reveal = app.folder_pane.cursor_scroll;
    let reveals = tree_scrolls(&ctx);
    let generation = app.items_generation;
    let folder = app.current_folder.clone();
    let child = egui::ViewportId::from_hash_of("folder-tree-input-foreign-child");
    let mut raw = egui::RawInput {
        viewport_id: child,
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(640.0, 480.0),
        )),
        time: Some((frame as f64 + 0.5) / 60.0),
        events: vec![
            egui::Event::WindowFocused(false),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Touch {
                device_id: egui::TouchDeviceId(769),
                id: egui::TouchId(1),
                phase: egui::TouchPhase::Cancel,
                pos,
                force: None,
            },
        ],
        ..Default::default()
    };
    raw.viewports.entry(egui::ViewportId::ROOT).or_default();
    raw.viewports.insert(
        child,
        egui::ViewportInfo {
            parent: Some(egui::ViewportId::ROOT),
            focused: Some(false),
            ..Default::default()
        },
    );
    ctx.run_ui(raw, |ui| {
        assert_eq!(
            ui.ctx().viewport_id(),
            child,
            "this is a real child egui input pass"
        );
        app.input_update_begin(ui.ctx());
        // A child callback cannot suspend the ROOT tree or drain its result owner.
        app.finish_folder_pane_presentation(ui.ctx(), false);
        assert_eq!(display_snapshot(&app), cache);
        assert!(
            !app.folder_pane.begin_label_input(root_pass),
            "child begin must not replace the root input pass"
        );
        assert!(app.folder_pane.take_label_results(root_pass).is_empty());
        assert!(app.folder_pane.label_gesture_requires_tree_focus());
        for row in &cache.2 {
            assert!(!app.folder_pane.label_pointer_activation_owned(row.1));
            assert!(!app.folder_pane.label_pointer_activation_owned(row.3));
        }
        assert_eq!(app.folder_pane.visible_rows(), rows);
        assert_eq!(app.folder_pane.has_focus, focus);
        assert_eq!(app.folder_pane.cursor_scroll, reveal);
        assert_eq!(tree_scrolls(&ctx), reveals);
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.current_folder, folder);
        assert!(!first.load(Ordering::Relaxed));
        assert!(Arc::ptr_eq(
            &first,
            &app.folder_pane_open_pending.as_ref().unwrap().cancel
        ));
    })
    .drop_without_applying_deltas();
    assert_eq!(
        ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT),
        root_completed
    );

    navigation_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, true)],
    );
    assert_label_expanded(&app, &b, true);
    assert!(!first.load(Ordering::Relaxed));
    assert!(Arc::ptr_eq(
        &first,
        &app.folder_pane_open_pending.as_ref().unwrap().cancel
    ));
    assert_eq!(app.folder_pane_open_pending.as_ref().unwrap().path, b);
    assert_eq!(app.folder_pane.has_focus, focus);
    assert_eq!(app.folder_pane.cursor_scroll, reveal);
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.current_folder, folder);
    navigation_tree_frame(&mut app, &ctx, &mut frame, vec![primary_event(pos, false)]);
    assert_label_expanded(&app, &b, true);
    assert!(!first.load(Ordering::Relaxed));
}

fn hover_layer_tree_frame(
    app: &mut App,
    ctx: &egui::Context,
    frame: &mut u64,
    events: Vec<egui::Event>,
    pos: egui::Pos2,
    overlay: Option<&str>,
) {
    *frame += 1;
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
            app.input_update_begin(ctx);
            let keyboard_nav = app.handle_keyboard(ctx);
            let (toolbar_fav_nav, address_nav) = app.render_toolbar(ui);
            let folder_pane_nav = app.render_folder_pane(ui).0;
            let grid_nav = app.render_grid(ui);
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
            let id = egui::Id::unique("folder-tree-hover-only-obstruction");
            let origin = pos - egui::vec2(40.0, 40.0);
            match overlay {
                Some("area") => {
                    egui::Area::new(id)
                        .order(egui::Order::Middle)
                        .fixed_pos(origin)
                        .interactable(true)
                        .sense(egui::Sense::hover())
                        .fade_in(false)
                        .show(ctx, |ui| {
                            ui.allocate_exact_size(egui::vec2(160.0, 100.0), egui::Sense::hover());
                        });
                }
                Some("window") => {
                    egui::Window::new("Hover-only window body")
                        .id(id)
                        .order(egui::Order::Middle)
                        .fixed_pos(origin)
                        // fixed_pos turns movement off; explicitly restore movement
                        // so the title-less Window's body has Area's DRAG-only sense.
                        .movable(true)
                        .title_bar(false)
                        .collapsible(false)
                        .resizable(false)
                        .fixed_size(egui::vec2(160.0, 100.0))
                        .fade_in(false)
                        .fade_out(false)
                        .show(ctx, |ui| {
                            ui.allocate_exact_size(egui::vec2(140.0, 80.0), egui::Sense::hover());
                        });
                }
                None => {}
                _ => unreachable!(),
            }
        },
    )
    .drop_without_applying_deltas();
}

fn hover_only_tree_layer_scenario(kind: &str) {
    let (mut app, ctx, mut frame, a, _, pos) = ordered_label_fixture();
    ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    let first = app
        .folder_pane_open_pending
        .as_ref()
        .expect("the first click navigates immediately")
        .cancel
        .clone();
    let (_hold, rx) = mpsc::channel();
    app.folder_pane_open_pending.as_mut().unwrap().rx = rx;
    let generation = app.items_generation;
    let folder = app.current_folder.clone();
    let first_release_time = frame as f64 / 60.0;
    assert_label_expanded(&app, &a, false);

    // Keep the production pane/toolbar/grid layout. Two draws establish the
    // foreign Area in the previous-frame hit-test data without moving A.
    for _ in 0..2 {
        hover_layer_tree_frame(&mut app, &ctx, &mut frame, vec![], pos, Some(kind));
    }
    let cache = app.folder_pane.label_display.as_ref().unwrap();
    assert!(
        cache.rect.contains(pos),
        "this is still within the tree's cached area"
    );
    let row = cache.rows.iter().find(|row| row.path == a).unwrap();
    assert!(
        row.label_rect.contains(pos),
        "the overlay covers A's original label position"
    );
    let tree_layer = ctx.read_response(row.label).unwrap().layer_id;
    let obstruction = egui::LayerId::new(
        egui::Order::Middle,
        egui::Id::unique("folder-tree-hover-only-obstruction"),
    );
    let area = egui::AreaState::load(&ctx, obstruction.id)
        .expect("the logical overlay Area was actually drawn");
    assert!(area.interactable && area.rect().contains(pos));
    assert_ne!(obstruction, tree_layer);
    assert_eq!(ctx.layer_id_at(pos), Some(obstruction));

    hover_layer_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![egui::Event::PointerMoved(pos), primary_event(pos, true)],
        pos,
        Some(kind),
    );
    let witnesses = ctx.pointer_button_targets();
    assert_eq!(witnesses.len(), 1);
    assert_eq!(
        (witnesses[0].0, witnesses[0].1, witnesses[0].2),
        (pos, egui::PointerButton::Primary, true)
    );
    assert_eq!(
        witnesses[0].3, None,
        "premise: the upper hover/drag layer has no click widget"
    );
    assert!(
        app.folder_pane
            .label_display
            .as_ref()
            .unwrap()
            .rect
            .contains(pos)
    );
    assert_eq!(ctx.layer_id_at(pos), Some(obstruction));
    assert_label_expanded(&app, &a, false);
    assert!(
        !app.folder_pane.label_gesture_requires_tree_focus(),
        "the foreign layer must cancel the old name gesture"
    );
    assert!(!first.load(Ordering::Relaxed));
    assert!(Arc::ptr_eq(
        &first,
        &app.folder_pane_open_pending.as_ref().unwrap().cancel
    ));
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.current_folder, folder);

    hover_layer_tree_frame(
        &mut app,
        &ctx,
        &mut frame,
        vec![primary_event(pos, false)],
        pos,
        Some(kind),
    );
    assert_label_expanded(&app, &a, false);
    assert!(!first.load(Ordering::Relaxed));
    assert!(Arc::ptr_eq(
        &first,
        &app.folder_pane_open_pending.as_ref().unwrap().cancel
    ));

    // Retire only the logical overlay, keeping all App/tree/navigation state.
    for _ in 0..2 {
        hover_layer_tree_frame(&mut app, &ctx, &mut frame, vec![], pos, None);
    }
    assert!(ctx.layer_id_at(pos).is_none_or(|layer| layer == tree_layer));
    assert!(
        frame as f64 / 60.0 - first_release_time
            < ctx.options(|options| options.input_options.max_double_click_delay)
    );
    pointer_tree_click(&mut app, &ctx, &mut frame, pos);
    assert_label_expanded(&app, &a, false);
    assert!(
        first.load(Ordering::Relaxed),
        "after overlay retirement this is a fresh, independent single click"
    );
    let pending = app
        .folder_pane_open_pending
        .as_ref()
        .expect("the fresh single click starts its own navigation");
    assert_eq!(pending.path, a);
    assert!(!Arc::ptr_eq(&first, &pending.cancel));
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.current_folder, folder);
}

#[test]
fn folder_pane_label_hover_only_upper_area_cancels_continuation() {
    hover_only_tree_layer_scenario("area");
}

#[test]
fn folder_pane_label_drag_only_upper_window_body_cancels_continuation() {
    hover_only_tree_layer_scenario("window");
}
