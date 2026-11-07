//! Exercise the production dialogs without native windows or normal-profile data.
use super::*;
use egui_kittest::kittest::Queryable;

fn assert_stable_width(title: &str, mut draw: impl FnMut(&egui::Context)) {
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let mut widths = Vec::new();
    for frame in 0..24 {
        let pos = egui::pos2(20.0 + frame as f32, 20.0);
        let mut events = vec![egui::Event::PointerMoved(pos)];
        if frame % 4 == 2 || frame % 4 == 3 {
            events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: frame % 4 == 2,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let _ = ctx
            .run_pass(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(2400.0, 1400.0),
                    )),
                    time: Some(frame as f64 / 60.0),
                    events,
                    ..Default::default()
                },
                |ctx| draw(ctx),
            )
            .drop_without_applying_deltas();
        widths.push(
            ctx.memory(|memory| memory.area_rect(egui::Id::unique(Some(title))))
                .expect("dialog is open")
                .width(),
        );
    }
    // Allow initial sizing, then check every input-driven pass, not just the last one.
    assert!(
        widths[3..]
            .iter()
            .all(|width| (width - widths[3]).abs() <= 0.1),
        "{title}: window widths by frame = {widths:?}"
    );
}

#[test]
fn effect_picker_width_stays_stable_with_all_effects() {
    let mut app = crate::app::setup_app_for_test();
    app.local_adjust_effect_picker_dialog_open = true;
    let mut query = String::new();
    assert_stable_width("加工内容を選択", |ctx| {
        app.draw_local_adjust_effect_picker_dialog(ctx, 0, &mut query, &mut None);
    });
}

#[test]
fn effect_picker_width_stays_stable_without_matching_effects() {
    let mut app = crate::app::setup_app_for_test();
    app.local_adjust_effect_picker_dialog_open = true;
    let mut query = "no-such-effect-1725".to_owned();
    assert_stable_width("加工内容を選択", |ctx| {
        app.draw_local_adjust_effect_picker_dialog(ctx, 0, &mut query, &mut None);
    });
}

#[test]
fn change_mask_picker_width_stays_stable() {
    let mut app = crate::app::setup_app_for_test();
    app.local_adjust_change_mask_dialog_open = true;
    let layers = vec![local_adjust_core::LocalAdjustmentLayer::new(
        "width test",
        local_adjust_core::LocalMask::Full,
        local_adjust_core::LocalEffect::None,
    )];
    assert_stable_width("マスク種類変更", |ctx| {
        app.draw_local_adjust_change_mask_dialog(ctx, &layers, 0, &mut None);
    });
}

#[test]
fn add_layer_picker_width_stays_stable() {
    let mut app = crate::app::setup_app_for_test();
    app.local_adjust_add_layer_dialog_open = true;
    assert_stable_width("補正レイヤーを追加", |ctx| {
        app.draw_local_adjust_add_layer_dialog(ctx, (100, 100), &mut None);
    });
}

#[test]
fn effect_picker_search_clear_and_selection_keep_the_row_inside_the_window() {
    assert_search_clear_and_selection(false);
}

#[test]
fn effect_picker_clear_wins_over_ime_commit_in_the_same_pass() {
    assert_search_clear_and_selection(true);
}

fn assert_search_clear_and_selection(ime_commit: bool) {
    let mut app = crate::app::setup_app_for_test();
    app.local_adjust_effect_picker_dialog_open = true;
    let mut harness = crate::test_ui::builder()
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .with_size(egui::vec2(1200.0, 800.0))
        .build_ui_state(
            |root, (app, query, selected)| {
                app.draw_local_adjust_effect_picker_dialog(root.ctx(), 0, query, selected);
            },
            (app, String::new(), None),
        );
    harness.run_steps(3);
    let window_rect = |ctx: &egui::Context| {
        ctx.memory(|memory| memory.area_rect(egui::Id::unique(Some("加工内容を選択"))))
            .unwrap()
    };
    let window = window_rect(&harness.ctx);
    let search = harness.get_by_role(egui::accesskit::Role::TextInput).rect();
    let clear = harness.get_by_label("×").rect();
    assert!(search.left() < clear.left() && search.right() < clear.left());
    assert!(window.contains_rect(search) && window.contains_rect(clear));
    harness
        .get_by_role(egui::accesskit::Role::TextInput)
        .click();
    harness.run_steps(2);
    harness.event(egui::Event::Text("no-such-effect-1725".into()));
    harness.run_steps(2);
    assert_eq!(harness.state().1, "no-such-effect-1725");
    assert!(harness.query_by_label("該当する効果がありません").is_some());
    if ime_commit {
        harness.event(egui::Event::Ime(egui::ImeEvent::Preedit {
            text: "確定".into(),
            active_range_chars: None,
        }));
        harness.run_steps(1);
    }
    let clear_pos = harness.get_by_label("×").rect().center();
    harness.event(egui::Event::PointerMoved(clear_pos));
    harness.event(egui::Event::PointerButton {
        pos: clear_pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(1);
    // A final text event and the clear release may arrive in the same pass.
    harness.event(if ime_commit {
        egui::Event::Ime(egui::ImeEvent::Commit("確定".into()))
    } else {
        egui::Event::Text("late input".into())
    });
    harness.event(egui::Event::PointerButton {
        pos: clear_pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(2);
    assert!(harness.state().1.is_empty());
    assert_eq!(window_rect(&harness.ctx).width(), window.width());
    let kind = EFFECT_GROUPS[0].kinds[0];
    harness.get_by_label(kind.picker_label()).click();
    harness.run_steps(2);
    assert_eq!(harness.state().2, Some((0, kind)));
    assert!(!harness.state().0.local_adjust_effect_picker_dialog_open);
}
