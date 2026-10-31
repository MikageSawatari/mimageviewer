use super::*;

fn button(pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos: egui::pos2(100.0, 100.0),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn front_popup_owner_ignores_another_viewports_focus_loss() {
    let ctx = egui::Context::default();
    let mut owner = FsFrontPointerOwnership::default();
    crate::test_ui::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            events: vec![button(true)],
            ..Default::default()
        },
        |ctx| {
            assert!(owner.capture(ctx, true, &[]).primary_owned);
        },
    ))
    .drop_without_applying_deltas();
    let other = egui::ViewportId::from_hash_of("other-front-viewer");
    let mut viewports = egui::ViewportIdMap::default();
    viewports.insert(other, egui::ViewportInfo::default());
    crate::test_ui::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            viewport_id: other,
            viewports,
            focused: false,
            ..Default::default()
        },
        |ctx| {
            assert!(!owner.capture(ctx, false, &[]).primary_owned);
            assert!(owner.owns_primary());
        },
    ))
    .drop_without_applying_deltas();
    crate::test_ui::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            focused: false,
            ..Default::default()
        },
        |ctx| {
            owner.capture(ctx, false, &[]);
            assert!(!owner.is_active());
        },
    ))
    .drop_without_applying_deltas();
}

#[test]
fn front_popup_old_release_and_fresh_press_in_one_pass_have_distinct_owners() {
    let ctx = egui::Context::default();
    let mut owner = FsFrontPointerOwnership::default();
    crate::test_ui::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            events: vec![button(true)],
            ..Default::default()
        },
        |ctx| {
            assert!(owner.capture(ctx, true, &[]).primary_owned);
        },
    ))
    .drop_without_applying_deltas();
    crate::test_ui::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            events: vec![button(false), button(true)],
            ..Default::default()
        },
        |ctx| {
            let frame = owner.capture(ctx, false, &[]);
            assert!(!frame.primary_owned);
            assert!(frame.owns_release(egui::PointerButton::Primary));
            assert!(!owner.is_active());
        },
    ))
    .drop_without_applying_deltas();
    crate::test_ui::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            events: vec![button(false), button(true)],
            ..Default::default()
        },
        |ctx| {
            assert!(owner.capture(ctx, true, &[]).primary_owned);
            assert!(owner.is_active());
        },
    ))
    .drop_without_applying_deltas();
}

#[test]
fn front_popup_old_touch_terminal_and_new_group_in_one_pass_are_distinct() {
    let ctx = egui::Context::default();
    let mut owner = FsFrontPointerOwnership::default();
    let touch = |phase| egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(1),
        phase,
        pos: egui::pos2(100.0, 100.0),
        force: None,
    };
    crate::test_ui::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            events: vec![touch(egui::TouchPhase::Start), button(true)],
            ..Default::default()
        },
        |ctx| {
            assert!(owner.capture(ctx, true, &[]).primary_owned);
        },
    ))
    .drop_without_applying_deltas();
    crate::test_ui::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            events: vec![
                touch(egui::TouchPhase::Cancel),
                touch(egui::TouchPhase::Start),
            ],
            ..Default::default()
        },
        |ctx| {
            let frame = owner.capture(ctx, false, &[]);
            assert!(!frame.primary_owned && !frame.touch_owned);
            assert!(!owner.is_active());
        },
    ))
    .drop_without_applying_deltas();
}

#[test]
fn front_popup_focus_loss_keeps_ordered_release_provenance_and_retires_the_owner() {
    for release_before_loss in [true, false] {
        let ctx = egui::Context::default();
        let mut owner = FsFrontPointerOwnership::default();
        crate::test_ui::headless_paint_output(ctx.run_ui(
            egui::RawInput {
                events: vec![button(true)],
                ..Default::default()
            },
            |ctx| {
                assert!(owner.capture(ctx, true, &[]).primary_owned);
            },
        ))
        .drop_without_applying_deltas();
        let events = if release_before_loss {
            vec![button(false), egui::Event::WindowFocused(false)]
        } else {
            vec![egui::Event::WindowFocused(false), button(false)]
        };
        crate::test_ui::headless_paint_output(ctx.run_ui(
            egui::RawInput {
                focused: false,
                events,
                ..Default::default()
            },
            |ctx| {
                assert!(!ctx.input(|input| input.raw.focused || input.focused));
                let frame = owner.capture(ctx, true, &[]);
                assert!(!owner.is_active());
                let releases: Vec<_> = frame
                    .events
                    .iter()
                    .filter(|entry| {
                        matches!(
                            entry.event,
                            egui::Event::PointerButton { pressed: false, .. }
                        )
                    })
                    .collect();
                assert_eq!(releases.len(), 1, "focus loss must not discard the release");
                assert_eq!(releases[0].owned, release_before_loss);
                assert!(
                    frame
                        .events
                        .iter()
                        .any(|entry| matches!(entry.event, egui::Event::WindowFocused(false)))
                );
            },
        ))
        .drop_without_applying_deltas();
    }
}

#[test]
fn front_popup_focus_round_trip_binds_a_new_stream_to_the_same_viewport() {
    let ctx = egui::Context::default();
    let mut owner = FsFrontPointerOwnership::default();
    crate::test_ui::headless_paint_output(ctx.run_ui(
        egui::RawInput {
            events: vec![
                egui::Event::WindowFocused(false),
                button(true),
                button(false),
                egui::Event::WindowFocused(true),
                button(true),
            ],
            ..Default::default()
        },
        |ctx| {
            let frame = owner.capture(ctx, true, &[]);
            assert!(
                !frame.events[1].owned,
                "a press before focus gain cannot start ownership"
            );
            assert!(frame.events[4].owned);
            assert!(owner.owns_primary_in(ctx.viewport_id()));
        },
    ))
    .drop_without_applying_deltas();
}
