use super::*;

fn frame(app: &mut App, ctx: &egui::Context, viewport: egui::Rect, events: Vec<egui::Event>) {
    ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(viewport),
            events,
            ..Default::default()
        },
        |ctx| {
            let result = app.handle_fs_key_input(ctx, 0, false);
            assert!(!result.close && matches!(result.page_nav, FsPageNav::None));
            ctx.root_ui(|root| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(root, |ui| {
                        app.draw_fs_overflow_panel(ui, ctx, viewport, false);
                    });
            })
        },
    )
    .drop_without_applying_deltas();
}

fn layout(ctx: &egui::Context) -> crate::test_ui::scroll_layout::ScrollLayout {
    let (inner, content, offset) = ctx.data(|data| {
        data.get_temp(egui::Id::unique("fs_overflow_scroll_test_layout"))
            .unwrap()
    });
    crate::test_ui::scroll_layout::ScrollLayout {
        inner,
        content,
        offset,
    }
}

fn slider_rect(ctx: &egui::Context) -> egui::Rect {
    ctx.data(|data| {
        data.get_temp(egui::Id::unique((
            "fs_slideshow_slider_test_rect",
            "ページ送り間隔:",
        )))
        .unwrap()
    })
}

fn pointer(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn touch(pos: egui::Pos2, phase: egui::TouchPhase) -> egui::Event {
    egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(1),
        phase,
        pos,
        force: None,
    }
}

#[test]
fn slideshow_settings_slider_mouse_and_touch_own_drag_and_save_once_at_release() {
    for is_touch in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
        app.slideshow_playing = true;
        let deadline = app.slideshow_next_at;
        app.settings.slideshow_interval_secs = 3.0;
        app.settings.save();
        let initial_generation = crate::settings::save_generation();
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        // Force body scrolling while leaving the first slider and the fixed Back visible.
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1093.0, 300.0));
        for _ in 0..4 {
            frame(&mut app, &ctx, viewport, vec![]);
        }
        let before = layout(&ctx);
        assert!(before.content.y > before.inner.height());
        let rect = slider_rect(&ctx);
        let start = egui::pos2(rect.left() + 20.0, rect.center().y);
        let end = start + egui::vec2(85.0, -20.0);
        let back = ctx.data(|data| {
            data.get_temp::<egui::Rect>(egui::Id::unique("fs_slideshow_back_test_rect"))
                .unwrap()
        });
        assert!(viewport.contains_rect(back));
        let mut press = vec![egui::Event::PointerMoved(start), pointer(start, true)];
        if is_touch {
            press.push(touch(start, egui::TouchPhase::Start));
        }
        frame(&mut app, &ctx, viewport, press);
        let mut movement = vec![egui::Event::PointerMoved(end)];
        if is_touch {
            movement.push(touch(end, egui::TouchPhase::Move));
        }
        frame(&mut app, &ctx, viewport, movement);
        assert_ne!(
            app.settings.slideshow_interval_secs, 3.0,
            "touch={is_touch}"
        );
        assert_eq!(
            layout(&ctx).offset,
            before.offset,
            "slider owns its drag, touch={is_touch}"
        );
        assert_eq!(
            crate::settings::save_generation(),
            initial_generation,
            "no per-frame persistence, touch={is_touch}"
        );
        assert_eq!(app.slideshow_next_at, deadline);
        let mut release = vec![pointer(end, false)];
        if is_touch {
            release.push(touch(end, egui::TouchPhase::End));
        }
        frame(&mut app, &ctx, viewport, release);
        assert_eq!(
            crate::settings::save_generation(),
            initial_generation + 1,
            "one release save, touch={is_touch}"
        );
        let stored = crate::settings_db::with_db_result(|db| db.load_into_settings()).unwrap();
        assert_eq!(
            stored.slideshow_interval_secs,
            app.settings.slideshow_interval_secs
        );
        frame(&mut app, &ctx, viewport, vec![]);
        assert_eq!(crate::settings::save_generation(), initial_generation + 1);
        assert_eq!(app.slideshow_next_at, deadline);
        assert!(app.slideshow_playing);
    }
}

#[test]
fn slideshow_settings_root_entry_end_choice_and_keyboard_persist_without_rescheduling() {
    use egui_kittest::kittest::Queryable;
    let mut app = crate::app::setup_app_for_test();
    app.fs_overflow_panel_state = FsOverflowPanelState::Root;
    app.slideshow_playing = true;
    let deadline = app.slideshow_next_at;
    app.settings.save();
    let initial_generation = crate::settings::save_generation();
    {
        let mut fonts_ready = false;
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .build_ui(|root| {
                let ctx = root.ctx().clone();
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(&ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(root, |ui| {
                        app.draw_fs_overflow_panel(ui, &ctx, ctx.content_rect(), false);
                    });
            });
        harness.run();
        harness.get_by_label("スライドショー").click();
        harness.run();
        harness.get_by_label("最後で停止").click();
        harness.run();
    }
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Slideshow);
    assert_eq!(app.settings.slideshow_end_action, SlideshowEndAction::Stop);
    assert_eq!(crate::settings::save_generation(), initial_generation + 1);
    assert_eq!(app.slideshow_next_at, deadline);
    // A focused slider receives keyboard changes through the same rendering/action boundary.
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1093.0, 614.0));
    for _ in 0..4 {
        frame(&mut app, &ctx, viewport, vec![]);
    }
    let pos = slider_rect(&ctx).left_center() + egui::vec2(20.0, 0.0);
    frame(
        &mut app,
        &ctx,
        viewport,
        vec![egui::Event::PointerMoved(pos), pointer(pos, true)],
    );
    frame(&mut app, &ctx, viewport, vec![pointer(pos, false)]);
    let generation = crate::settings::save_generation();
    let interval = app.settings.slideshow_interval_secs;
    // The rail is keyboard-focusable through keyboard/accessibility navigation;
    // pointer dragging alone does not give egui's Sense::drag widget focus.
    let rail_id = ctx.data(|data| {
        data.get_temp::<egui::Id>(egui::Id::unique((
            "fs_slideshow_slider_test_id",
            "ページ送り間隔:",
        )))
        .unwrap()
    });
    ctx.memory_mut(|memory| memory.request_focus(rail_id));
    frame(
        &mut app,
        &ctx,
        viewport,
        vec![egui::Event::Key {
            key: egui::Key::ArrowRight,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_ne!(app.settings.slideshow_interval_secs, interval);
    assert_eq!(crate::settings::save_generation(), generation + 1);
    assert_eq!(app.slideshow_next_at, deadline);
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Slideshow);

    // Actual pointer entry into the numeric editor also keeps keys and text local.
    frame(
        &mut app,
        &ctx,
        viewport,
        vec![egui::Event::Key {
            key: egui::Key::ArrowRight,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    // Read the rail's actual widget rect, rather than the union returned by Slider.
    let rail = ctx.read_response(rail_id).unwrap().rect;
    let whole = slider_rect(&ctx);
    let pos = egui::pos2((rail.right() + whole.right()) * 0.5, rail.center().y);
    frame(
        &mut app,
        &ctx,
        viewport,
        vec![egui::Event::PointerMoved(pos), pointer(pos, true)],
    );
    frame(&mut app, &ctx, viewport, vec![pointer(pos, false)]);
    frame(&mut app, &ctx, viewport, vec![]);
    let focused = ctx.memory(|memory| memory.focused());
    assert!(
        focused.is_some_and(|id| id != rail_id),
        "numeric click={pos:?}, slider={:?}, focused={focused:?}, rail={rail_id:?}, response={:?}",
        slider_rect(&ctx),
        focused
            .and_then(|id| ctx.read_response(id))
            .map(|response| response.rect)
    );
    let generation = crate::settings::save_generation();
    frame(
        &mut app,
        &ctx,
        viewport,
        vec![egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
        }],
    );
    frame(
        &mut app,
        &ctx,
        viewport,
        vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            },
            egui::Event::Text("12.0".into()),
        ],
    );
    frame(
        &mut app,
        &ctx,
        viewport,
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(app.settings.slideshow_interval_secs, 12.0);
    assert_eq!(crate::settings::save_generation(), generation + 1);
    assert_eq!(app.slideshow_next_at, deadline);
    assert_eq!(app.fs_overflow_panel_state, FsOverflowPanelState::Slideshow);
}

#[test]
fn slideshow_settings_header_and_back_stay_fixed_when_body_scrolls() {
    use crate::test_ui::scroll_layout::HeightExpectation;
    let mut app = crate::app::setup_app_for_test();
    app.fs_overflow_panel_state = FsOverflowPanelState::Slideshow;
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    for size in [egui::vec2(1093.0, 614.0), egui::vec2(1093.0, 300.0)] {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let panel = fs_overflow_panel_rect(viewport, FsOverflowPanelState::Slideshow).unwrap();
        for _ in 0..4 {
            frame(&mut app, &ctx, viewport, vec![]);
        }
        let before = layout(&ctx);
        let back = ctx.data(|data| {
            data.get_temp::<egui::Rect>(egui::Id::unique("fs_slideshow_back_test_rect"))
                .unwrap()
        });
        assert!(viewport.contains_rect(panel));
        assert!(panel.contains_rect(back));
        assert!(before.inner.top() >= back.bottom());
        before.assert_height(
            if size.y > 400.0 {
                HeightExpectation::Fits
            } else {
                HeightExpectation::Scrolls
            },
            panel.intersect(viewport),
            ctx.pixels_per_point(),
        );
        frame(
            &mut app,
            &ctx,
            viewport,
            vec![
                egui::Event::PointerMoved(before.inner.center()),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -4000.0),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                },
            ],
        );
        for _ in 0..35 {
            frame(&mut app, &ctx, viewport, vec![]);
        }
        if size.y < 400.0 {
            layout(&ctx).assert_scrolled_to_end(before, ctx.pixels_per_point());
        }
        assert_eq!(
            ctx.data(|data| data
                .get_temp::<egui::Rect>(egui::Id::unique("fs_slideshow_back_test_rect"))
                .unwrap()),
            back
        );
    }
}
