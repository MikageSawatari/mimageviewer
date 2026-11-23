//! §1.471: close uses the existing navigation and foreground lifetimes.
use super::page_turn_submission_tests::{
    adopt_interrupted_target, ready_app, seed_interrupted_leaf, seed_leaf,
};
use super::*;

fn setup(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    page_turn::install(ctx);
    install_fs_navigator_input_tracking(ctx);
    let mut app = ready_app(ctx, SpreadMode::Single);
    app.settings.fullscreen_primary_click =
        crate::settings::FullscreenPrimaryClickMode::DoubleClickClose;
    app.settings.fullscreen_navigator_visible = false;
    app.settings.fullscreen_side_panel_mode = crate::settings::FsSidePanelMode::ClickToShow;
    app
}

fn button(pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos: egui::pos2(300.0, 200.0),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn double_click() -> Vec<egui::Event> {
    [true, false, true, false].into_iter().map(button).collect()
}

fn frame(ctx: &egui::Context, events: Vec<egui::Event>, mut draw: impl FnMut(&mut egui::Ui)) {
    let time = ctx.input(|input| input.time) + 0.01;
    // A promoted touch cursor must follow that contact's real coordinates.
    // Injecting the mouse fixture's centre move into its live stream is a drag.
    let events = if events
        .iter()
        .any(|event| matches!(event, egui::Event::Touch { .. }))
    {
        events
    } else {
        std::iter::once(egui::Event::PointerMoved(egui::pos2(300.0, 200.0)))
            .chain(events)
            .collect()
    };
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 400.0),
            )),
            time: Some(time),
            focused: true,
            events,
            ..Default::default()
        },
        |ui| draw(ui),
    )
    .drop_without_applying_deltas();
}

fn close_with_navigation(app: &mut App, ui: &mut egui::Ui, key_nav: bool) {
    let ctx = ui.ctx().clone();
    let idx = app.fullscreen_idx.unwrap();
    let keys = key_nav.then(|| {
        crate::keyboard_input::cache_keyboard_owner(
            &ctx,
            crate::keyboard_input::KeyboardOwner::ApplicationShortcut {
                scope: crate::keyboard_input::ShortcutScope::new(
                    ctx.viewport_id(),
                    crate::keyboard_input::ShortcutSurface::Fullscreen,
                ),
            },
        );
        let keys = app.handle_fs_key_input(&ctx, idx, false);
        assert!(
            !keys.page_nav.is_none(),
            "the simultaneous key must supply real navigation"
        );
        keys
    });
    let (result, operation) = super::wheel_frame_tests::actual_merge(app, ui, false);
    assert!(result.close && result.primary_click_close);
    app.handle_fs_navigation_with_operation(
        &ctx,
        result.close,
        false,
        None,
        None,
        None,
        keys.map_or(result.page_nav, |keys| keys.page_nav),
        None,
        idx,
        operation,
    );
    assert_eq!(app.fullscreen_idx, None);
    assert!(!app.page_turn_barrier_active());
    assert!(!app.page_turn_completion_active());
    assert!(!app.commit_page_turn_continuation(&ctx));
    assert!(!app.fs_front_pointer.clicks.previous_close_candidate);
    assert_eq!(app.fs_front_pointer.clicks.current_accepted, 0);
}

#[test]
fn double_click_close_during_animation_retires_pending_continuation() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = setup(&ctx);
    frame(&ctx, vec![], |ui| {
        super::wheel_frame_tests::actual_merge(&mut app, ui, false);
    });
    seed_interrupted_leaf(&ctx, &mut app, 2);
    assert!(app.page_turn_completion_active());
    frame(&ctx, double_click(), |ui| {
        close_with_navigation(&mut app, ui, false)
    });
}

#[test]
fn double_click_close_after_committed_continuation_leaves_no_navigation() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = setup(&ctx);
    frame(&ctx, vec![], |ui| {
        super::wheel_frame_tests::actual_merge(&mut app, ui, false);
    });
    seed_interrupted_leaf(&ctx, &mut app, 2);
    adopt_interrupted_target(&ctx, &mut app);
    frame(&ctx, double_click(), |ui| {
        assert!(matches!(
            app.begin_page_turn_viewer_frame(ui.ctx(), false, None),
            PageTurnFrameEntry::Continue {
                committed: true,
                ..
            }
        ));
        assert_eq!(app.fullscreen_idx, Some(2));
        close_with_navigation(&mut app, ui, false);
    });
}

#[test]
fn double_click_close_with_navigation_key_clears_animation_and_key_navigation() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    crate::key_input::install_operation_input_plugin(&ctx);
    let mut app = setup(&ctx);
    frame(&ctx, vec![], |ui| {
        super::wheel_frame_tests::actual_merge(&mut app, ui, false);
    });
    seed_leaf(&ctx, &mut app);
    let mut events = vec![egui::Event::Key {
        key: egui::Key::ArrowRight,
        physical_key: Some(egui::Key::ArrowRight),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }];
    events.extend(double_click());
    frame(&ctx, events, |ui| close_with_navigation(&mut app, ui, true));
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
}

#[test]
fn double_click_close_reopen_does_not_inherit_the_closing_pair() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = setup(&ctx);
    frame(&ctx, vec![], |ui| {
        super::wheel_frame_tests::actual_merge(&mut app, ui, false);
    });
    frame(&ctx, double_click(), |ui| {
        close_with_navigation(&mut app, ui, false)
    });
    app.open_fullscreen(0, crate::app::HistoryTrigger::UserChosen);
    frame(&ctx, vec![button(true), button(false)], |ui| {
        let (result, _) = super::wheel_frame_tests::actual_merge(&mut app, ui, false);
        assert!(!result.close);
        assert!(result.page_nav.is_none());
        assert_eq!(app.fs_front_pointer.clicks.current_accepted, 1);
    });
}

#[test]
fn double_click_close_overview_and_similar_preview_retire_the_canvas_column() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    for overview in [true, false] {
        let ctx = egui::Context::default();
        let mut app = setup(&ctx);
        app.settings.video_in_window_mode = true;
        app.viewer_presentation = ViewerPresentation::MainWindow;
        app.native_video_in_window_active = true;
        app.page_identity_index = Arc::new(
            crate::app::file_type_resume::PreparedPageIdentityIndex::prepare(
                &app.items,
                None,
                app.settings.file_type_visibility.clone(),
            ),
        );
        frame(&ctx, vec![], |ui| {
            super::wheel_frame_tests::actual_merge(&mut app, ui, false);
        });
        frame(&ctx, vec![button(true), button(false)], |ui| {
            let (result, _) = super::wheel_frame_tests::actual_merge(&mut app, ui, false);
            assert!(!result.close);
        });
        assert!(app.fs_front_pointer.clicks.previous_close_candidate);
        if overview {
            assert!(app.open_page_overview(
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0)),
                Default::default(),
            ));
        } else {
            super::tests::begin_navigator_preview_for_test(&mut app, &ctx, 0);
            assert!(app.similar_panel.preview.has_active_gesture());
        }
        frame(&ctx, vec![], |ui| {
            app.capture_fs_front_pointer_input(ui.ctx(), ui.max_rect());
        });
        assert!(!app.fs_front_pointer.clicks.previous_close_candidate);
        if overview {
            app.close_page_overview();
        } else {
            app.similar_panel.preview.end_gesture();
        }
        frame(&ctx, vec![button(true), button(false)], |ui| {
            let (result, _) = super::wheel_frame_tests::actual_merge(&mut app, ui, false);
            assert!(!result.close);
            assert!(result.page_nav.is_none());
        });
    }
}

#[test]
fn double_click_close_mode_preserves_touch_page_tap_and_suppresses_promoted_click() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = setup(&ctx);
    app.settings.touch_still_chrome_learned = true;
    let pos = egui::pos2(500.0, 200.0);
    let touch = |phase| egui::Event::Touch {
        device_id: egui::TouchDeviceId(471),
        id: egui::TouchId(1),
        phase,
        pos,
        force: None,
    };
    let pointer = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&ctx, vec![], |ui| {
        super::wheel_frame_tests::actual_merge(&mut app, ui, false);
    });
    frame(
        &ctx,
        vec![
            touch(egui::TouchPhase::Start),
            egui::Event::PointerMoved(pos),
            pointer(true),
        ],
        |ui| {
            app.frame_counter += 1;
            let (result, _) = super::wheel_frame_tests::actual_merge(&mut app, ui, false);
            assert!(!result.close);
        },
    );
    frame(
        &ctx,
        vec![
            touch(egui::TouchPhase::End),
            egui::Event::PointerMoved(pos),
            pointer(false),
        ],
        |ui| {
            app.frame_counter += 1;
            let (result, _) = super::wheel_frame_tests::actual_merge(&mut app, ui, false);
            assert!(!result.close);
            assert_eq!(result.page_nav, FsPageNav::Delta(1));
            assert!(!app.fs_front_pointer.clicks.previous_close_candidate);
        },
    );
}

#[cfg(windows)]
#[test]
fn double_click_close_f11_and_f12_retire_the_old_foreground_column() {
    let _input = crate::key_input::lock_test_input();
    for detached in [false, true] {
        let ctx = egui::Context::default();
        let mut app = setup(&ctx);
        frame(&ctx, vec![], |ui| {
            super::wheel_frame_tests::actual_merge(&mut app, ui, false);
        });
        frame(&ctx, vec![button(true), button(false)], |ui| {
            let (result, _) = super::wheel_frame_tests::actual_merge(&mut app, ui, false);
            assert!(!result.close);
        });
        assert!(app.fs_front_pointer.clicks.previous_close_candidate);
        if detached {
            app.settings.detached_viewer_open_images_in_window = false;
            app.toggle_detached_viewer_mode();
        } else {
            app.toggle_still_window_mode();
        }
        assert!(!app.fs_front_pointer.clicks.previous_close_candidate);
        assert_eq!(app.fs_front_pointer.clicks.current_accepted, 0);
    }
}

#[cfg(windows)]
#[test]
fn double_click_close_in_window_context_preserves_main_and_sibling() {
    let _input = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = setup(&ctx);
    let main_items = app.items.clone();
    let main_generation = app.items_generation;
    let sibling = app.build_window_context_for_test(14712, |owner| {
        owner.items = vec![GridItem::Image(PathBuf::from("c:/471/sibling.png"))];
        owner.fullscreen_idx = Some(0);
        owner.fs_zoom = 2.5;
        owner.fs_pan = egui::vec2(10.0, 20.0);
        owner.items_generation = 712;
    });
    let closing = app.build_window_context_for_test(14711, |owner| {
        owner.items = vec![GridItem::Image(PathBuf::from("c:/471/closing.png"))];
        owner.visible_indices = vec![0];
        owner.fullscreen_idx = Some(0);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
        owner.fs_zoom = 1.0;
        owner.fs_pan = egui::Vec2::ZERO;
    });
    app.with_window_context_for_test(closing, |owner| {
        owner.begin_active_detached_session(14711, crate::app::DetachedSource::Image);
        frame(&ctx, vec![], |ui| {
            super::wheel_frame_tests::actual_merge(owner, ui, false);
        });
        frame(&ctx, double_click(), |ui| {
            close_with_navigation(owner, ui, false)
        });
    });
    assert_eq!(app.items, main_items);
    assert_eq!(app.items_generation, main_generation);
    assert_eq!(app.fullscreen_idx, Some(0));
    app.with_window_context_for_test(sibling, |owner| {
        assert_eq!(owner.fullscreen_idx, Some(0));
        assert_eq!(owner.items_generation, 712);
        assert_eq!(owner.fs_zoom, 2.5);
        assert_eq!(owner.fs_pan, egui::vec2(10.0, 20.0));
        assert_eq!(
            owner.items,
            vec![GridItem::Image(PathBuf::from("c:/471/sibling.png"))]
        );
    });
}
