use super::*;
use std::path::{Path, PathBuf};

fn test_context() -> egui::Context {
    let ctx = egui::Context::default();
    install_ime_input_policy(&ctx);
    ctx
}

fn key_event(key: egui::Key) -> egui::Event {
    key_event_state(key, true)
}

fn key_event_state(key: egui::Key, pressed: bool) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn viewport_raw_input(viewport_id: egui::ViewportId, events: Vec<egui::Event>) -> egui::RawInput {
    let mut input = egui::RawInput {
        viewport_id,
        events,
        ..Default::default()
    };
    input.viewports.insert(
        viewport_id,
        egui::ViewportInfo {
            parent: (viewport_id != egui::ViewportId::ROOT).then_some(egui::ViewportId::ROOT),
            ..Default::default()
        },
    );
    input
}

#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn explicit_viewport_ime_read_is_read_only_and_viewport_isolated() {
    let ctx = test_context();
    let viewport_id = egui::ViewportId::from_hash_of("gamepad-ime-fullscreen");
    ctx.begin_pass(viewport_raw_input(
        viewport_id,
        vec![
            egui::Event::Ime(egui::ImeEvent::Enabled),
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "あ".to_owned(),
                active_range_chars: None,
            }),
        ],
    ));
    assert!(ime_input_active(&ctx));
    ctx.end_pass().drop_without_applying_deltas();

    let before = ctx
        .with_plugin(|plugin: &mut ImeInputPlugin| {
            plugin.viewports.get(&viewport_id).copied().unwrap()
        })
        .unwrap();
    let mut root_input = viewport_raw_input(egui::ViewportId::ROOT, Vec::new());
    root_input.viewports.insert(viewport_id, Default::default());
    ctx.begin_pass(root_input);
    assert!(ime_input_active_in_viewport(&ctx, viewport_id));
    let after_read = ctx
        .with_plugin(|plugin: &mut ImeInputPlugin| {
            plugin.viewports.get(&viewport_id).copied().unwrap()
        })
        .unwrap();
    assert_eq!(after_read, before);
    assert!(!ime_input_active_in_viewport(&ctx, egui::ViewportId::ROOT));
    assert!(!ime_input_active(&ctx));
    let after_root_update = ctx
        .with_plugin(|plugin: &mut ImeInputPlugin| {
            plugin.viewports.get(&viewport_id).copied().unwrap()
        })
        .unwrap();
    assert_eq!(after_root_update, before);
    ctx.end_pass().drop_without_applying_deltas();
}

#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn current_viewport_ime_update_still_tracks_root_composition() {
    let ctx = test_context();
    ctx.begin_pass(viewport_raw_input(
        egui::ViewportId::ROOT,
        vec![
            egui::Event::Ime(egui::ImeEvent::Enabled),
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "あ".to_owned(),
                active_range_chars: None,
            }),
        ],
    ));
    assert!(ime_input_active(&ctx));
    assert!(ime_input_active_in_viewport(&ctx, egui::ViewportId::ROOT));
    ctx.end_pass().drop_without_applying_deltas();
}

#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn pending_native_events_project_without_mutating_plugin_state() {
    let ctx = test_context();
    establish_composition(&ctx, egui::ViewportId::ROOT);
    let before = ctx
        .with_plugin(|plugin: &mut ImeInputPlugin| {
            plugin
                .viewports
                .get(&egui::ViewportId::ROOT)
                .copied()
                .unwrap()
        })
        .unwrap();
    for pending in [
        vec![egui::Event::Ime(egui::ImeEvent::Commit("あ".to_owned()))],
        vec![egui::Event::WindowFocused(false)],
        vec![
            egui::Event::WindowFocused(false),
            egui::Event::Ime(egui::ImeEvent::Disabled),
        ],
        vec![
            egui::Event::Ime(egui::ImeEvent::Disabled),
            egui::Event::WindowFocused(false),
        ],
    ] {
        assert!(!ime_composing_with_pending_events(
            &ctx,
            egui::ViewportId::ROOT,
            &pending,
        ));
    }
    let after = ctx
        .with_plugin(|plugin: &mut ImeInputPlugin| {
            plugin
                .viewports
                .get(&egui::ViewportId::ROOT)
                .copied()
                .unwrap()
        })
        .unwrap();
    assert_eq!(after, before);
}

#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn establish_composition(ctx: &egui::Context, viewport_id: egui::ViewportId) {
    ctx.begin_pass(viewport_raw_input(
        viewport_id,
        vec![
            egui::Event::Ime(egui::ImeEvent::Enabled),
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "あ".to_owned(),
                active_range_chars: None,
            }),
        ],
    ));
    assert!(ime_input_active(ctx));
    ctx.end_pass().drop_without_applying_deltas();
}

fn empty_preedit_count(ctx: &egui::Context) -> usize {
    ctx.input(|input| {
        input
            .events
            .iter()
            .filter(|event| {
                matches!(event, egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) if text.is_empty())
            })
            .count()
    })
}

#[test]
fn composing_normal_key_presses_and_releases_are_preserved() {
    let ctx = test_context();
    let viewport_id = egui::ViewportId::from_hash_of("ime-escape-filter");
    establish_composition(&ctx, viewport_id);

    // These are ordinary bridge-delivered keys, not IME-handled Process presses.
    // Composition by itself must not make the normalizer suppress them.
    for key in [egui::Key::Enter, egui::Key::Escape, egui::Key::Tab] {
        let events = vec![key_event_state(key, true), key_event_state(key, false)];
        ctx.begin_pass(viewport_raw_input(viewport_id, events.clone()));
        ctx.input(|input| assert_eq!(input.events, events));
        assert_eq!(empty_preedit_count(&ctx), 0);
        ctx.end_pass().drop_without_applying_deltas();
    }
}

#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn noncomposing_and_post_commit_keys_are_preserved_without_activity() {
    for same_pass in [true, false] {
        for key in [egui::Key::Enter, egui::Key::Escape, egui::Key::J] {
            let ctx = test_context();
            establish_composition(&ctx, egui::ViewportId::ROOT);
            let mut events = vec![
                egui::Event::Ime(egui::ImeEvent::Commit("確定".to_owned())),
                egui::Event::Ime(egui::ImeEvent::Disabled),
            ];
            if same_pass {
                events.push(key_event(key));
            } else {
                ctx.begin_pass(viewport_raw_input(egui::ViewportId::ROOT, events));
                assert!(!ime_input_active(&ctx));
                ctx.end_pass().drop_without_applying_deltas();
                events = vec![key_event(key)];
            }
            ctx.begin_pass(viewport_raw_input(egui::ViewportId::ROOT, events));
            assert!(!ime_input_active(&ctx));
            assert!(ctx.input(|input| input.events.contains(&key_event(key))));
            assert_eq!(empty_preedit_count(&ctx), 0);
            ctx.end_pass().drop_without_applying_deltas();
        }
    }
}

#[test]
fn viewport_destroy_and_focus_loss_discard_unterminated_composition() {
    let ctx = test_context();
    let child = egui::ViewportId::from_hash_of("reopened-ime");
    establish_composition(&ctx, child);
    // A ROOT pass reports the child no longer exists, with no terminal IME event.
    ctx.begin_pass(viewport_raw_input(egui::ViewportId::ROOT, vec![]));
    assert!(!ime_input_active_in_viewport(&ctx, child));
    ctx.end_pass().drop_without_applying_deltas();
    ctx.begin_pass(viewport_raw_input(child, vec![]));
    assert!(!ime_input_active(&ctx));
    ctx.end_pass().drop_without_applying_deltas();

    for event_only in [false, true] {
        establish_composition(&ctx, child);
        let mut input = viewport_raw_input(child, vec![]);
        if event_only {
            input.events.push(egui::Event::WindowFocused(false));
        } else {
            input.focused = false;
        }
        ctx.begin_pass(input);
        assert!(!ime_input_active(&ctx));
        ctx.end_pass().drop_without_applying_deltas();
        ctx.begin_pass(viewport_raw_input(child, vec![]));
        assert!(!ime_input_active(&ctx));
        ctx.end_pass().drop_without_applying_deltas();
    }
}

#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn composing_disabled_inserts_one_empty_preedit_before_disabled() {
    let ctx = test_context();
    let viewport_id = egui::ViewportId::from_hash_of("ime-disabled-cancel");
    establish_composition(&ctx, viewport_id);

    ctx.begin_pass(viewport_raw_input(
        viewport_id,
        vec![egui::Event::Ime(egui::ImeEvent::Disabled)],
    ));
    let _ = ime_input_active(&ctx);
    assert_eq!(empty_preedit_count(&ctx), 1);
    ctx.input(|input| {
        assert!(matches!(
            input.events.as_slice(),
            [
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }),
                egui::Event::Ime(egui::ImeEvent::Disabled),
            ] if text.is_empty()
        ));
    });

    let _ = ime_input_active(&ctx);
    assert_eq!(empty_preedit_count(&ctx), 1);
    ctx.end_pass().drop_without_applying_deltas();
}

/// The cancellation sequence captured from `[ime-raw]` on 2026-08-01, with
/// the Process press and its physical Esc release both excluded by egui-winit:
/// `Disabled, Disabled, Commit(""), Disabled, Disabled`.
///
/// The empty commit is a cancellation, not a confirmation: egui's commit
/// handler skips `delete_selected` for an empty prediction, so without the
/// inserted cancel the preedit stays in the buffer as confirmed text.
#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn windows_escape_cancel_sequence_clears_the_preedit() {
    let ctx = test_context();
    let viewport_id = egui::ViewportId::from_hash_of("ime-windows-escape-cancel");
    establish_composition(&ctx, viewport_id);

    ctx.begin_pass(viewport_raw_input(
        viewport_id,
        vec![
            egui::Event::Ime(egui::ImeEvent::Disabled),
            egui::Event::Ime(egui::ImeEvent::Disabled),
            egui::Event::Ime(egui::ImeEvent::Commit(String::new())),
            egui::Event::Ime(egui::ImeEvent::Disabled),
            egui::Event::Ime(egui::ImeEvent::Disabled),
        ],
    ));
    let _ = ime_input_active(&ctx);

    assert_eq!(
        empty_preedit_count(&ctx),
        1,
        "the empty commit must not be mistaken for a confirmation"
    );
    ctx.input(|input| {
        assert!(
            !input.events.iter().any(|event| matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::Escape,
                    ..
                }
            )),
            "the cancelling Esc must not reach egui focus handling"
        );
        let first_ime = input
            .events
            .iter()
            .position(|event| matches!(event, egui::Event::Ime(_)))
            .expect("the batch carries IME events");
        assert!(
            matches!(
                &input.events[first_ime],
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) if text.is_empty()
            ),
            "the cancel must land while the preedit is still selected"
        );
    });
    ctx.end_pass().drop_without_applying_deltas();
}

/// A non-empty commit is a real confirmation and must be left alone.
#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn non_empty_commit_with_disabled_does_not_insert_cancel_preedit() {
    let ctx = test_context();
    let viewport_id = egui::ViewportId::from_hash_of("ime-real-commit");
    establish_composition(&ctx, viewport_id);

    ctx.begin_pass(viewport_raw_input(
        viewport_id,
        vec![
            egui::Event::Ime(egui::ImeEvent::Commit("確定".to_owned())),
            egui::Event::Ime(egui::ImeEvent::Disabled),
        ],
    ));
    let _ = ime_input_active(&ctx);
    assert_eq!(empty_preedit_count(&ctx), 0);
    ctx.end_pass().drop_without_applying_deltas();
}

#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn commit_and_disabled_does_not_insert_cancel_preedit() {
    let ctx = test_context();
    let viewport_id = egui::ViewportId::from_hash_of("ime-commit-disabled");
    establish_composition(&ctx, viewport_id);

    ctx.begin_pass(viewport_raw_input(
        viewport_id,
        vec![
            egui::Event::Ime(egui::ImeEvent::Commit("あ".to_owned())),
            egui::Event::Ime(egui::ImeEvent::Disabled),
        ],
    ));
    let _ = ime_input_active(&ctx);
    assert_eq!(empty_preedit_count(&ctx), 0);
    ctx.end_pass().drop_without_applying_deltas();
}

#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn disabled_without_local_composition_preserves_events_and_other_viewport() {
    let ctx = test_context();
    let composing_viewport = egui::ViewportId::from_hash_of("ime-isolated-composing");
    establish_composition(&ctx, composing_viewport);

    let mut root_input = viewport_raw_input(
        egui::ViewportId::ROOT,
        vec![
            key_event(egui::Key::Escape),
            egui::Event::Ime(egui::ImeEvent::Disabled),
        ],
    );
    root_input
        .viewports
        .insert(composing_viewport, Default::default());
    ctx.begin_pass(root_input);
    let _ = ime_input_active(&ctx);
    assert_eq!(empty_preedit_count(&ctx), 0);
    assert!(ctx.input(|input| input.events.contains(&key_event(egui::Key::Escape))));
    assert!(ime_input_active_in_viewport(&ctx, composing_viewport,));
    ctx.end_pass().drop_without_applying_deltas();
}

#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn raw_multiline_text_edit_keeps_focus_and_cancels_preedit_across_delayed_disabled() {
    let ctx = test_context();
    let field_id = egui::Id::unique("ime-escape-raw-multiline");
    let mut body = String::new();
    let mut request_focus = true;
    let draw = |ui: &mut egui::Ui, body: &mut String, request_focus: &mut bool| {
        egui::CentralPanel::default().show(ui, |ui| {
            let response = ui.add(egui::TextEdit::multiline(body).id(field_id));
            if std::mem::take(request_focus) {
                response.request_focus();
            }
        });
    };

    ctx.run_ui(Default::default(), |ui| {
        draw(ui, &mut body, &mut request_focus);
    })
    .drop_without_applying_deltas();
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));

    ctx.run_ui(
        viewport_raw_input(
            egui::ViewportId::ROOT,
            vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "あ".to_owned(),
                    active_range_chars: None,
                }),
            ],
        ),
        |ui| draw(ui, &mut body, &mut request_focus),
    )
    .drop_without_applying_deltas();
    assert_eq!(body, "あ");
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));

    // Neither half of the IME-processed Escape pair reaches the winit event queue.
    ctx.run_ui(viewport_raw_input(egui::ViewportId::ROOT, vec![]), |ui| {
        let ctx = &ui.ctx().clone();
        assert!(!ctx.input(|input| input.events.iter().any(|event| matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Escape,
                ..
            }
        ))));
        draw(ui, &mut body, &mut request_focus);
    })
    .drop_without_applying_deltas();
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));

    let mut cancel_preedit_count = 0;
    ctx.run_ui(
        viewport_raw_input(
            egui::ViewportId::ROOT,
            vec![egui::Event::Ime(egui::ImeEvent::Disabled)],
        ),
        |ui| {
            let ctx = &ui.ctx().clone();
            cancel_preedit_count = empty_preedit_count(ctx);
            draw(ui, &mut body, &mut request_focus);
        },
    )
    .drop_without_applying_deltas();
    assert_eq!(cancel_preedit_count, 1);
    assert!(body.is_empty());
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));
}

#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn assert_text_edit_terminal_around_focus_loss(terminal: egui::ImeEvent, expected: &str) {
    for event_only in [false, true] {
        for focus_first in [false, true] {
            for delayed in [false, true] {
                let ctx = test_context();
                let id = egui::Id::unique("focus-loss-preedit");
                let mut text = "元".to_owned();
                let mut request = true;
                ctx.run_ui(Default::default(), |ui| {
                    draw_recovery_field(ui, id, &mut text, Some(&mut request), false);
                })
                .drop_without_applying_deltas();
                ctx.run_ui(
                    viewport_raw_input(
                        egui::ViewportId::ROOT,
                        vec![
                            egui::Event::Ime(egui::ImeEvent::Enabled),
                            egui::Event::Ime(egui::ImeEvent::Preedit {
                                text: "あ".into(),
                                active_range_chars: None,
                            }),
                        ],
                    ),
                    |ui| {
                        draw_recovery_field(ui, id, &mut text, None, false);
                    },
                )
                .drop_without_applying_deltas();
                assert_eq!(text, "元あ");
                assert!(ime_input_active(&ctx));

                let terminal_event = egui::Event::Ime(terminal.clone());
                let mut focus_input = viewport_raw_input(egui::ViewportId::ROOT, vec![]);
                if event_only {
                    focus_input.events.push(egui::Event::WindowFocused(false));
                } else {
                    focus_input.focused = false;
                }
                let inputs = if delayed {
                    let terminal_input =
                        viewport_raw_input(egui::ViewportId::ROOT, vec![terminal_event]);
                    if focus_first {
                        vec![focus_input, terminal_input]
                    } else {
                        vec![terminal_input, focus_input]
                    }
                } else {
                    if focus_first {
                        focus_input.events.push(terminal_event);
                    } else {
                        focus_input.events.insert(0, terminal_event);
                    }
                    vec![focus_input]
                };
                for input in inputs {
                    ctx.run_ui(input, |ui| {
                        let ctx = &ui.ctx().clone();
                        assert!(!ime_input_active(ctx));
                        draw_recovery_field(ui, id, &mut text, None, false);
                    })
                    .drop_without_applying_deltas();
                }
                assert_eq!(
                    text, expected,
                    "event_only={event_only}, focus_first={focus_first}, delayed={delayed}"
                );
                // Regaining focus and repeated terminal events must not revive
                // the shortcut claim or delete the already finalized buffer.
                ctx.run_ui(
                    viewport_raw_input(
                        egui::ViewportId::ROOT,
                        vec![
                            egui::Event::WindowFocused(true),
                            egui::Event::Ime(egui::ImeEvent::Disabled),
                        ],
                    ),
                    |ui| {
                        let ctx = &ui.ctx().clone();
                        assert!(!ime_input_active(ctx));
                        assert_eq!(empty_preedit_count(ctx), 0);
                        draw_recovery_field(ui, id, &mut text, None, false);
                    },
                )
                .drop_without_applying_deltas();
                assert_eq!(text, expected);
            }
        }
    }
}

#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn text_edit_cancels_preedit_with_disabled_before_or_after_focus_loss() {
    for terminal in [
        egui::ImeEvent::Disabled,
        egui::ImeEvent::Commit(String::new()),
    ] {
        assert_text_edit_terminal_around_focus_loss(terminal, "元");
    }
}

#[test]
fn text_edit_preserves_commit_before_or_after_focus_loss() {
    assert_text_edit_terminal_around_focus_loss(egui::ImeEvent::Commit("確定".into()), "元確定");
}

fn draw_text_fields(
    ui: &mut egui::Ui,
    first_id: egui::Id,
    second_id: egui::Id,
    first: &mut String,
    second: &mut String,
    first_focus_request: &mut bool,
) {
    egui::CentralPanel::default().show(ui, |ui| {
        let _ = show_singleline(ui, first, Some(first_focus_request), |edit| {
            edit.id(first_id)
        });
        let _ = show_singleline(ui, second, None, |edit| edit.id(second_id));
    });
}

#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn ime_candidate_tab_keeps_the_editing_field_focus() {
    let ctx = test_context();
    let first_id = egui::Id::unique("ime_tab_first");
    let second_id = egui::Id::unique("ime_tab_second");
    let mut first = String::new();
    let mut second = String::new();
    let mut focus_request = true;
    ctx.run_ui(Default::default(), |ui| {
        draw_text_fields(
            ui,
            first_id,
            second_id,
            &mut first,
            &mut second,
            &mut focus_request,
        );
    })
    .drop_without_applying_deltas();

    ctx.run_ui(
        egui::RawInput {
            events: vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "あ".to_owned(),
                    active_range_chars: None,
                }),
            ],
            ..Default::default()
        },
        |ui| {
            draw_text_fields(
                ui,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        },
    )
    .drop_without_applying_deltas();
    ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "亜".to_owned(),
                active_range_chars: None,
            })],
            ..Default::default()
        },
        |ui| {
            draw_text_fields(
                ui,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        },
    )
    .drop_without_applying_deltas();

    assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
    assert!(ctx.egui_wants_keyboard_input());
    assert_eq!(first, "亜");
    assert!(second.is_empty());
}

#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn ime_start_is_sampled_before_text_edit_consumes_events() {
    let ctx = test_context();
    let viewport_id = egui::ViewportId::from_hash_of("ime_focus_fullscreen_viewport");
    let first_id = egui::Id::unique("ime_start_focus_gain_first");
    let second_id = egui::Id::unique("ime_start_focus_gain_second");
    let mut first = String::new();
    let mut second = String::new();
    let mut focus_request = true;
    let raw_input = |events| {
        let mut input = egui::RawInput {
            viewport_id,
            events,
            ..Default::default()
        };
        input.viewports.insert(
            viewport_id,
            egui::ViewportInfo {
                parent: Some(egui::ViewportId::ROOT),
                ..Default::default()
            },
        );
        input
    };

    ctx.run_ui(raw_input(Vec::new()), |ui| {
        draw_text_fields(
            ui,
            first_id,
            second_id,
            &mut first,
            &mut second,
            &mut focus_request,
        );
    })
    .drop_without_applying_deltas();
    let mut ime_events_remained_after_text_edit = true;
    ctx.run_ui(
        raw_input(vec![
            egui::Event::Ime(egui::ImeEvent::Enabled),
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "あ".to_owned(),
                active_range_chars: None,
            }),
        ]),
        |ui| {
            let ctx = &ui.ctx().clone();
            egui::CentralPanel::default().show(ui, |ui| {
                let event_queue = ctx.clone();
                let _ = show_singleline(ui, &mut first, Some(&mut focus_request), |edit| {
                    // egui TextEdit removes Ime events on a focus transition. Keep that
                    // behavior explicit in this fixture so the helper's sampling order is
                    // tested independently of egui's private focus-history timing.
                    event_queue.input_mut(|input| {
                        input
                            .events
                            .retain(|event| !matches!(event, egui::Event::Ime(_)));
                    });
                    edit.id(first_id)
                });
                let _ = show_singleline(ui, &mut second, None, |edit| edit.id(second_id));
            });
            ime_events_remained_after_text_edit = ctx.input(|input| {
                input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Ime(_)))
            });
        },
    )
    .drop_without_applying_deltas();

    assert!(
        !ime_events_remained_after_text_edit,
        "fixture must reproduce TextEdit consuming the IME-start events"
    );
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));

    // A normal Esc deliberately exercises the retained focus-recovery path.
    // This is not the Process press used to cancel an IME composition.
    ctx.run_ui(raw_input(vec![key_event(egui::Key::Escape)]), |ui| {
        draw_text_fields(
            ui,
            first_id,
            second_id,
            &mut first,
            &mut second,
            &mut focus_request,
        );
    })
    .drop_without_applying_deltas();

    assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
}

#[test]
fn text_cursor_arrow_keys_keep_the_editing_field_focus() {
    let ctx = egui::Context::default();
    let first_id = egui::Id::unique("arrow_first");
    let second_id = egui::Id::unique("arrow_second");
    let mut first = String::from("abc");
    let mut second = String::new();
    let mut focus_request = true;
    ctx.run_ui(Default::default(), |ui| {
        draw_text_fields(
            ui,
            first_id,
            second_id,
            &mut first,
            &mut second,
            &mut focus_request,
        );
    })
    .drop_without_applying_deltas();
    ctx.run_ui(Default::default(), |ui| {
        draw_text_fields(
            ui,
            first_id,
            second_id,
            &mut first,
            &mut second,
            &mut focus_request,
        );
    })
    .drop_without_applying_deltas();
    ctx.run_ui(
        egui::RawInput {
            events: vec![key_event(egui::Key::ArrowDown)],
            ..Default::default()
        },
        |ui| {
            draw_text_fields(
                ui,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        },
    )
    .drop_without_applying_deltas();

    assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
}

#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn ime_activity_without_a_focus_key_does_not_undo_focus_change() {
    let ctx = test_context();
    let first_id = egui::Id::unique("ime_click_first");
    let second_id = egui::Id::unique("ime_click_second");
    let mut first = String::new();
    let mut second = String::new();
    let mut focus_request = true;
    ctx.run_ui(Default::default(), |ui| {
        draw_text_fields(
            ui,
            first_id,
            second_id,
            &mut first,
            &mut second,
            &mut focus_request,
        );
    })
    .drop_without_applying_deltas();
    ctx.memory_mut(|memory| memory.request_focus(second_id));
    ctx.run_ui(
        egui::RawInput {
            events: vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "あ".to_owned(),
                    active_range_chars: None,
                }),
            ],
            ..Default::default()
        },
        |ui| {
            draw_text_fields(
                ui,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        },
    )
    .drop_without_applying_deltas();

    assert_eq!(ctx.memory(|memory| memory.focused()), Some(second_id));
}

#[test]
#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn ime_enter_commit_and_escape_cancel_keep_the_editing_field_focus() {
    for key in [egui::Key::Enter, egui::Key::Escape] {
        let ctx = test_context();
        let first_id = egui::Id::unique(("ime_focus_key_first", key));
        let second_id = egui::Id::unique(("ime_focus_key_second", key));
        let mut first = String::new();
        let mut second = String::new();
        let mut focus_request = true;
        ctx.run_ui(Default::default(), |ui| {
            draw_text_fields(
                ui,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        })
        .drop_without_applying_deltas();
        ctx.run_ui(
            egui::RawInput {
                events: vec![
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit {
                        text: "あ".to_owned(),
                        active_range_chars: None,
                    }),
                ],
                ..Default::default()
            },
            |ui| {
                draw_text_fields(
                    ui,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(first, "あ");
        let terminal_ime_event = if key == egui::Key::Enter {
            egui::Event::Ime(egui::ImeEvent::Commit("あ".to_owned()))
        } else {
            egui::Event::Ime(egui::ImeEvent::Disabled)
        };
        ctx.run_ui(
            egui::RawInput {
                events: vec![
                    terminal_ime_event,
                    egui::Event::Ime(egui::ImeEvent::Disabled),
                ],
                ..Default::default()
            },
            |ui| {
                draw_text_fields(
                    ui,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        )
        .drop_without_applying_deltas();

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
        assert_eq!(first, if key == egui::Key::Enter { "あ" } else { "" });
        assert!(second.is_empty());
    }
}

fn draw_recovery_field(
    ui: &mut egui::Ui,
    id: egui::Id,
    text: &mut String,
    request: Option<&mut bool>,
    lose_focus: bool,
) -> egui::Response {
    egui::CentralPanel::default()
        .show(ui, |ui| {
            let ctx = &ui.ctx().clone();
            show_singleline(ui, text, request, |edit| {
                if lose_focus {
                    ctx.memory_mut(|memory| memory.surrender_focus(id));
                }
                edit.id(id)
            })
            .response
            .response
        })
        .inner
}

#[allow(
    deprecated,
    reason = "Exercise legacy Enabled/Disabled compatibility notifications retained for Windows cancellation"
)]
fn recover_with_alt_disabled(
    ctx: &egui::Context,
    id: egui::Id,
    text: &mut String,
    mut request: Option<&mut bool>,
) {
    let mut alt_j = key_event(egui::Key::J);
    if let egui::Event::Key { modifiers, .. } = &mut alt_j {
        modifiers.alt = true;
    }
    ctx.run_ui(
        viewport_raw_input(
            egui::ViewportId::ROOT,
            vec![egui::Event::Ime(egui::ImeEvent::Disabled), alt_j],
        ),
        |ui| {
            let ctx = &ui.ctx().clone();
            let response = draw_recovery_field(ui, id, text, request.as_deref_mut(), true);
            assert!(response.has_focus(), "transient Alt loss must be restored");
            assert!(request.as_deref().is_none_or(|request| !request));
            assert_eq!(
                ctx.data(|data| data.get_temp::<u64>(focus_retry_id(ctx.viewport_id(), id))),
                Some(ctx.cumulative_pass_nr())
            );
        },
    )
    .drop_without_applying_deltas();
}

#[test]
fn focus_retry_preserves_normal_enter_escape_and_other_widget_focus() {
    for external_latch in [false, true] {
        for next in [egui::Key::Enter, egui::Key::Escape, egui::Key::J] {
            let ctx = test_context();
            let id = egui::Id::unique(("retry-field", external_latch, next));
            let other = egui::Id::unique("other-field");
            let mut text = "text".to_owned();
            let mut caller_request = true;
            ctx.run_ui(Default::default(), |ui| {
                draw_recovery_field(ui, id, &mut text, Some(&mut caller_request), false);
            })
            .drop_without_applying_deltas();
            assert!(!caller_request);
            assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));
            recover_with_alt_disabled(
                &ctx,
                id,
                &mut text,
                external_latch.then_some(&mut caller_request),
            );
            assert!(!caller_request);
            ctx.run_ui(
                viewport_raw_input(egui::ViewportId::ROOT, vec![key_event(next)]),
                |ui| {
                    let ctx = &ui.ctx().clone();
                    if next == egui::Key::J {
                        ctx.memory_mut(|memory| memory.request_focus(other));
                    }
                    let request = external_latch.then_some(&mut caller_request);
                    let response = draw_recovery_field(ui, id, &mut text, request, false);
                    assert!(!response.has_focus());
                    if next == egui::Key::J {
                        assert_eq!(ctx.memory(|memory| memory.focused()), Some(other));
                    } else {
                        assert!(response.lost_focus());
                        assert!(ctx.memory(|memory| memory.focused()).is_none());
                    }
                    assert!(!caller_request);
                    let retry = ctx
                        .data(|data| data.get_temp::<u64>(focus_retry_id(ctx.viewport_id(), id)));
                    assert!(retry.is_none());
                    // Explicit caller requests remain unconditional, even on Enter/Escape.
                    caller_request = true;
                },
            )
            .drop_without_applying_deltas();
            ctx.run_ui(Default::default(), |ui| {
                let ctx = &ui.ctx().clone();
                draw_recovery_field(ui, id, &mut text, Some(&mut caller_request), false);
                assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));
            })
            .drop_without_applying_deltas();
            assert!(!caller_request);
        }
    }
}

#[test]
fn focus_retry_expires_when_field_is_hidden_before_next_pass() {
    for external_latch in [false, true] {
        let ctx = test_context();
        let id = egui::Id::unique(("hidden-retry-field", external_latch));
        let mut text = "text".to_owned();
        let mut caller_request = true;
        ctx.run_ui(Default::default(), |ui| {
            draw_recovery_field(ui, id, &mut text, Some(&mut caller_request), false);
        })
        .drop_without_applying_deltas();
        recover_with_alt_disabled(
            &ctx,
            id,
            &mut text,
            external_latch.then_some(&mut caller_request),
        );
        assert!(!caller_request);
        for _ in 0..2 {
            ctx.run_ui(Default::default(), |ui| {
                egui::CentralPanel::default().show(ui, |_| {});
            })
            .drop_without_applying_deltas();
        }
        ctx.run_ui(
            viewport_raw_input(egui::ViewportId::ROOT, vec![key_event(egui::Key::J)]),
            |ui| {
                let ctx = &ui.ctx().clone();
                let request = external_latch.then_some(&mut caller_request);
                let response = draw_recovery_field(ui, id, &mut text, request, false);
                assert!(
                    !response.has_focus(),
                    "a stale retry must not refocus a reopened field"
                );
                assert!(
                    ctx.data(|data| data.get_temp::<u64>(focus_retry_id(ctx.viewport_id(), id)))
                        .is_none()
                );
            },
        )
        .drop_without_applying_deltas();
        assert!(!caller_request);
    }
}

#[test]
fn preedit_active_character_range_survives_shared_normalization() {
    let preedit = egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "\u{5909}\u{63db}\u{3042}".to_owned(),
        active_range_chars: Some(1..3),
    });
    let mut events = vec![preedit.clone()];
    let mut state = ImeFocusState::Inactive;
    normalize_ime_input(&mut events, &mut state);
    assert_eq!(events, vec![preedit]);
    assert_eq!(state, ImeFocusState::Composing);
}

#[test]
fn delete_surrounding_preserves_composition_and_focus_loss_obligation() {
    let deletion = egui::Event::Ime(egui::ImeEvent::DeleteSurrounding {
        before_chars: 1,
        after_chars: 2,
    });
    for initial in [
        ImeFocusState::Inactive,
        ImeFocusState::Composing,
        ImeFocusState::UnfocusedComposition,
    ] {
        let mut state = initial;
        let mut events = vec![deletion.clone()];
        normalize_ime_input(&mut events, &mut state);
        assert_eq!(events, vec![deletion.clone()]);
        assert_eq!(state, initial);
    }
}

#[test]
#[allow(
    deprecated,
    reason = "Disabled must still cancel a composition after a surrounding-text edit"
)]
fn delete_surrounding_does_not_hide_pending_composition_from_clipboard_gate() {
    let ctx = test_context();
    establish_composition(&ctx, egui::ViewportId::ROOT);
    let deletion = egui::Event::Ime(egui::ImeEvent::DeleteSurrounding {
        before_chars: 1,
        after_chars: 0,
    });
    assert!(ime_composing_with_pending_events(
        &ctx,
        egui::ViewportId::ROOT,
        std::slice::from_ref(&deletion),
    ));
    ctx.begin_pass(viewport_raw_input(egui::ViewportId::ROOT, vec![deletion]));
    assert!(ime_input_active(&ctx));
    ctx.end_pass().drop_without_applying_deltas();
    ctx.begin_pass(viewport_raw_input(
        egui::ViewportId::ROOT,
        vec![egui::Event::Ime(egui::ImeEvent::Disabled)],
    ));
    assert!(!ime_input_active(&ctx));
    assert_eq!(empty_preedit_count(&ctx), 1);
    ctx.end_pass().drop_without_applying_deltas();
}

struct RawTextEditExemption {
    path: &'static str,
    line_anchor: &'static str,
    expected_occurrences: usize,
    reason: &'static str,
}

const RAW_TEXT_EDIT_EXEMPTIONS: &[RawTextEditExemption] = &[
    RawTextEditExemption {
        path: "src/lib.rs",
        line_anchor: "let output = configure(",
        expected_occurrences: 1,
        reason: "共有 helper 自身が raw TextEdit を構築する境界",
    },
    RawTextEditExemption {
        path: "src/lib.rs",
        line_anchor: "let response = ui.add(configure(",
        expected_occurrences: 1,
        reason: "共有 helper 自身が raw TextEdit を構築する境界",
    },
    RawTextEditExemption {
        path: "src/lib.rs",
        line_anchor: "let response = ui.add_sized(size, configure(",
        expected_occurrences: 1,
        reason: "共有 helper 自身が raw TextEdit を構築する境界",
    },
    RawTextEditExemption {
        path: "src/tests.rs",
        line_anchor: "multiline(body).id(field_id)",
        expected_occurrences: 1,
        reason: "pass 前 IME plugin を raw 複数行 editor で直接検証する test fixture",
    },
];

fn collect_rust_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src directory") {
        let entry = entry.expect("read src entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_sources(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

#[test]
fn raw_text_edits_are_ime_aware_or_explicitly_exempt() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src_dir = manifest_dir.join("src");
    let mut files = Vec::new();
    collect_rust_sources(&src_dir, &mut files);
    files.sort();

    let raw_patterns = [
        ["TextEdit::", "singleline("].concat(),
        ["TextEdit::", "multiline("].concat(),
        [".text_edit_", "singleline("].concat(),
        [".text_edit_", "multiline("].concat(),
    ];
    let mut exemption_matches = vec![0usize; RAW_TEXT_EDIT_EXEMPTIONS.len()];
    let mut violations = Vec::new();

    for path in files {
        let relative = path
            .strip_prefix(&manifest_dir)
            .expect("source under manifest directory")
            .to_string_lossy()
            .replace('\\', "/");
        let source = std::fs::read_to_string(&path).expect("read Rust source as UTF-8");
        let lines: Vec<&str> = source.lines().collect();
        for (line_idx, line) in lines.iter().enumerate() {
            if !raw_patterns.iter().any(|pattern| line.contains(pattern)) {
                continue;
            }
            let matches: Vec<usize> = RAW_TEXT_EDIT_EXEMPTIONS
                .iter()
                .enumerate()
                .filter_map(|(idx, exemption)| {
                    (exemption.path == relative
                        && line.contains(exemption.line_anchor)
                        && !exemption.reason.trim().is_empty())
                    .then_some(idx)
                })
                .collect();
            match matches.as_slice() {
                [idx] => exemption_matches[*idx] += 1,
                [] => violations.push(format!("{relative}:{}: {}", line_idx + 1, line.trim())),
                _ => violations.push(format!(
                    "{relative}:{}: 複数の exemption に一致: {}",
                    line_idx + 1,
                    line.trim()
                )),
            }
        }
    }

    for (idx, exemption) in RAW_TEXT_EDIT_EXEMPTIONS.iter().enumerate() {
        assert_eq!(
            exemption_matches[idx], exemption.expected_occurrences,
            "IME raw TextEdit exemption が現ソースと一致しません: {} / {} ({})",
            exemption.path, exemption.line_anchor, exemption.reason
        );
    }
    assert!(
        violations.is_empty(),
        "IME focus policy を通らない raw TextEdit があります:\n{}\n自由入力欄は \
         egui_ime の helper を使ってください。意図的に対象外とする場合は、\
         tests.rs の RAW_TEXT_EDIT_EXEMPTIONS に短い理由付きで追加してください。",
        violations.join("\n")
    );
}
