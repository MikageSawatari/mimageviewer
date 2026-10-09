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
fn explicit_viewport_ime_read_is_read_only_and_viewport_isolated() {
    let ctx = test_context();
    let viewport_id = egui::ViewportId::from_hash_of("gamepad-ime-fullscreen");
    ctx.begin_pass(viewport_raw_input(
        viewport_id,
        vec![
            egui::Event::Ime(egui::ImeEvent::Enabled),
            egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
        ],
    ));
    assert!(ime_input_active(&ctx));
    let _ = ctx.end_pass();

    let before = ctx
        .with_plugin(|plugin: &mut ImeInputPlugin| {
            plugin.viewports.get(&viewport_id).copied().unwrap()
        })
        .unwrap();
    ctx.begin_pass(viewport_raw_input(egui::ViewportId::ROOT, Vec::new()));
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
    let _ = ctx.end_pass();
}

#[test]
fn current_viewport_ime_update_still_tracks_root_composition() {
    let ctx = test_context();
    ctx.begin_pass(viewport_raw_input(
        egui::ViewportId::ROOT,
        vec![
            egui::Event::Ime(egui::ImeEvent::Enabled),
            egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
        ],
    ));
    assert!(ime_input_active(&ctx));
    assert!(ime_input_active_in_viewport(&ctx, egui::ViewportId::ROOT));
    let _ = ctx.end_pass();
}

#[test]
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
    let pending = vec![egui::Event::Ime(egui::ImeEvent::Commit("あ".to_owned()))];

    assert!(!ime_composing_with_pending_events(
        &ctx,
        egui::ViewportId::ROOT,
        &pending,
    ));
    assert!(ime_input_active_with_pending_events(
        &ctx,
        egui::ViewportId::ROOT,
        &pending,
    ));
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

fn establish_composition(ctx: &egui::Context, viewport_id: egui::ViewportId) {
    ctx.begin_pass(viewport_raw_input(
        viewport_id,
        vec![
            egui::Event::Ime(egui::ImeEvent::Enabled),
            egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
        ],
    ));
    assert!(ime_input_active(ctx));
    let _ = ctx.end_pass();
}

fn empty_preedit_count(ctx: &egui::Context) -> usize {
    ctx.input(|input| {
        input
            .events
            .iter()
            .filter(|event| {
                matches!(event, egui::Event::Ime(egui::ImeEvent::Preedit(text)) if text.is_empty())
            })
            .count()
    })
}

#[test]
fn composing_escape_press_is_removed_but_release_is_preserved() {
    let ctx = test_context();
    let viewport_id = egui::ViewportId::from_hash_of("ime-escape-filter");
    establish_composition(&ctx, viewport_id);

    ctx.begin_pass(viewport_raw_input(
        viewport_id,
        vec![
            key_event_state(egui::Key::Escape, true),
            key_event_state(egui::Key::Escape, false),
        ],
    ));
    ctx.input(|input| {
        assert_eq!(
            input.events,
            vec![key_event_state(egui::Key::Escape, false)]
        );
    });
    assert_eq!(empty_preedit_count(&ctx), 0);
    let _ = ctx.end_pass();
}

#[test]
fn noncomposing_and_ime_grace_escape_presses_are_preserved() {
    let ctx = test_context();
    let viewport_id = egui::ViewportId::from_hash_of("ime-escape-grace");

    ctx.begin_pass(viewport_raw_input(
        viewport_id,
        vec![key_event(egui::Key::Escape)],
    ));
    assert!(ctx.input(|input| input.events == vec![key_event(egui::Key::Escape)]));
    assert_eq!(empty_preedit_count(&ctx), 0);
    let _ = ctx.end_pass();

    establish_composition(&ctx, viewport_id);
    ctx.begin_pass(viewport_raw_input(
        viewport_id,
        vec![egui::Event::Ime(egui::ImeEvent::Disabled)],
    ));
    assert_eq!(empty_preedit_count(&ctx), 1);
    assert!(ime_input_active(&ctx), "Disabled starts the shortcut grace");
    assert!(
        !ctx.with_plugin(|plugin: &mut ImeInputPlugin| {
            plugin.viewports.get(&viewport_id).unwrap().composing
        })
        .unwrap()
    );
    let mut grace_state = ctx
        .with_plugin(|plugin: &mut ImeInputPlugin| *plugin.viewports.get(&viewport_id).unwrap())
        .unwrap();
    let _ = ctx.end_pass();

    let mut events = vec![key_event(egui::Key::Escape)];
    assert!(
        grace_state.input_active_at(Instant::now()),
        "the 300ms grace is still active"
    );
    normalize_ime_input(&mut events, &mut grace_state, Instant::now());
    assert_eq!(events, vec![key_event(egui::Key::Escape)]);
    assert!(!events.iter().any(
        |event| matches!(event, egui::Event::Ime(egui::ImeEvent::Preedit(text)) if text.is_empty())
    ));
}

#[test]
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
                egui::Event::Ime(egui::ImeEvent::Preedit(text)),
                egui::Event::Ime(egui::ImeEvent::Disabled),
            ] if text.is_empty()
        ));
    });

    let _ = ime_input_active(&ctx);
    assert_eq!(empty_preedit_count(&ctx), 1);
    let _ = ctx.end_pass();
}

/// The sequence Windows actually delivers when Esc cancels a composition,
/// captured from `[ime-raw]` tracing on 2026-08-01:
/// `EscPress, Disabled, Disabled, Commit(""), Disabled, Disabled`.
///
/// The empty commit is a cancellation, not a confirmation: egui's commit
/// handler skips `delete_selected` for an empty prediction, so without the
/// inserted cancel the preedit stays in the buffer as confirmed text.
#[test]
fn windows_escape_cancel_sequence_clears_the_preedit() {
    let ctx = test_context();
    let viewport_id = egui::ViewportId::from_hash_of("ime-windows-escape-cancel");
    establish_composition(&ctx, viewport_id);

    ctx.begin_pass(viewport_raw_input(
        viewport_id,
        vec![
            key_event(egui::Key::Escape),
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
                    pressed: true,
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
                egui::Event::Ime(egui::ImeEvent::Preedit(text)) if text.is_empty()
            ),
            "the cancel must land while the preedit is still selected"
        );
    });
    let _ = ctx.end_pass();
}

/// A non-empty commit is a real confirmation and must be left alone.
#[test]
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
    let _ = ctx.end_pass();
}

#[test]
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
    let _ = ctx.end_pass();
}

#[test]
fn disabled_without_local_composition_preserves_events_and_other_viewport() {
    let ctx = test_context();
    let composing_viewport = egui::ViewportId::from_hash_of("ime-isolated-composing");
    establish_composition(&ctx, composing_viewport);

    ctx.begin_pass(viewport_raw_input(
        egui::ViewportId::ROOT,
        vec![
            key_event(egui::Key::Escape),
            egui::Event::Ime(egui::ImeEvent::Disabled),
        ],
    ));
    let _ = ime_input_active(&ctx);
    assert_eq!(empty_preedit_count(&ctx), 0);
    assert!(ctx.input(|input| input.events.contains(&key_event(egui::Key::Escape))));
    assert!(ime_input_active_in_viewport(&ctx, composing_viewport,));
    let _ = ctx.end_pass();
}

#[test]
fn raw_multiline_text_edit_keeps_focus_and_cancels_preedit_across_delayed_disabled() {
    let ctx = test_context();
    let field_id = egui::Id::new("ime-escape-raw-multiline");
    let mut body = String::new();
    let mut request_focus = true;
    let draw = |ctx: &egui::Context, body: &mut String, request_focus: &mut bool| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let response = ui.add(egui::TextEdit::multiline(body).id(field_id));
            if std::mem::take(request_focus) {
                response.request_focus();
            }
        });
    };

    let _ = ctx.run(Default::default(), |ctx| {
        draw(ctx, &mut body, &mut request_focus);
    });
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));

    let _ = ctx.run(
        viewport_raw_input(
            egui::ViewportId::ROOT,
            vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
            ],
        ),
        |ctx| draw(ctx, &mut body, &mut request_focus),
    );
    assert_eq!(body, "あ");
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));

    let mut escape_press_seen = true;
    let _ = ctx.run(
        viewport_raw_input(
            egui::ViewportId::ROOT,
            vec![
                key_event_state(egui::Key::Escape, true),
                key_event_state(egui::Key::Escape, false),
            ],
        ),
        |ctx| {
            escape_press_seen = ctx.input(|input| {
                input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::Escape,
                            pressed: true,
                            ..
                        }
                    )
                })
            });
            draw(ctx, &mut body, &mut request_focus);
        },
    );
    assert!(!escape_press_seen);
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));

    let mut cancel_preedit_count = 0;
    let _ = ctx.run(
        viewport_raw_input(
            egui::ViewportId::ROOT,
            vec![egui::Event::Ime(egui::ImeEvent::Disabled)],
        ),
        |ctx| {
            cancel_preedit_count = empty_preedit_count(ctx);
            draw(ctx, &mut body, &mut request_focus);
        },
    );
    assert_eq!(cancel_preedit_count, 1);
    assert!(body.is_empty());
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));
}

fn draw_text_fields(
    ctx: &egui::Context,
    first_id: egui::Id,
    second_id: egui::Id,
    first: &mut String,
    second: &mut String,
    first_focus_request: &mut bool,
) {
    egui::CentralPanel::default().show(ctx, |ui| {
        let _ = show_singleline(ui, first, Some(first_focus_request), |edit| {
            edit.id(first_id)
        });
        let _ = show_singleline(ui, second, None, |edit| edit.id(second_id));
    });
}

#[test]
fn ime_tab_restores_the_editing_field_focus() {
    let ctx = test_context();
    let first_id = egui::Id::new("ime_tab_first");
    let second_id = egui::Id::new("ime_tab_second");
    let mut first = String::new();
    let mut second = String::new();
    let mut focus_request = true;
    let _ = ctx.run(Default::default(), |ctx| {
        draw_text_fields(
            ctx,
            first_id,
            second_id,
            &mut first,
            &mut second,
            &mut focus_request,
        );
    });

    let _ = ctx.run(
        egui::RawInput {
            events: vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
            ],
            ..Default::default()
        },
        |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            events: vec![key_event(egui::Key::Tab)],
            ..Default::default()
        },
        |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        },
    );

    assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
    assert!(ctx.wants_keyboard_input());
}

#[test]
fn ime_start_is_sampled_before_text_edit_consumes_events() {
    let ctx = test_context();
    let viewport_id = egui::ViewportId::from_hash_of("ime_focus_fullscreen_viewport");
    let first_id = egui::Id::new("ime_start_focus_gain_first");
    let second_id = egui::Id::new("ime_start_focus_gain_second");
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

    let _ = ctx.run(raw_input(Vec::new()), |ctx| {
        draw_text_fields(
            ctx,
            first_id,
            second_id,
            &mut first,
            &mut second,
            &mut focus_request,
        );
    });
    let mut ime_events_remained_after_text_edit = true;
    let _ = ctx.run(
        raw_input(vec![
            egui::Event::Ime(egui::ImeEvent::Enabled),
            egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
        ]),
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
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
    );

    assert!(
        !ime_events_remained_after_text_edit,
        "fixture must reproduce TextEdit consuming the IME-start events"
    );
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));

    let _ = ctx.run(raw_input(vec![key_event(egui::Key::Tab)]), |ctx| {
        draw_text_fields(
            ctx,
            first_id,
            second_id,
            &mut first,
            &mut second,
            &mut focus_request,
        );
    });

    assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
}

#[test]
fn text_cursor_arrow_keys_keep_the_editing_field_focus() {
    let ctx = egui::Context::default();
    let first_id = egui::Id::new("arrow_first");
    let second_id = egui::Id::new("arrow_second");
    let mut first = String::from("abc");
    let mut second = String::new();
    let mut focus_request = true;
    let _ = ctx.run(Default::default(), |ctx| {
        draw_text_fields(
            ctx,
            first_id,
            second_id,
            &mut first,
            &mut second,
            &mut focus_request,
        );
    });
    let _ = ctx.run(Default::default(), |ctx| {
        draw_text_fields(
            ctx,
            first_id,
            second_id,
            &mut first,
            &mut second,
            &mut focus_request,
        );
    });
    let _ = ctx.run(
        egui::RawInput {
            events: vec![key_event(egui::Key::ArrowDown)],
            ..Default::default()
        },
        |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        },
    );

    assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
}

#[test]
fn ime_activity_without_a_focus_key_does_not_undo_focus_change() {
    let ctx = test_context();
    let first_id = egui::Id::new("ime_click_first");
    let second_id = egui::Id::new("ime_click_second");
    let mut first = String::new();
    let mut second = String::new();
    let mut focus_request = true;
    let _ = ctx.run(Default::default(), |ctx| {
        draw_text_fields(
            ctx,
            first_id,
            second_id,
            &mut first,
            &mut second,
            &mut focus_request,
        );
    });
    ctx.memory_mut(|memory| memory.request_focus(second_id));
    let _ = ctx.run(
        egui::RawInput {
            events: vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
            ],
            ..Default::default()
        },
        |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        },
    );

    assert_eq!(ctx.memory(|memory| memory.focused()), Some(second_id));
}

#[test]
fn ime_enter_and_escape_restore_the_editing_field_focus() {
    for key in [egui::Key::Enter, egui::Key::Escape] {
        let ctx = test_context();
        let first_id = egui::Id::new(("ime_focus_key_first", key));
        let second_id = egui::Id::new(("ime_focus_key_second", key));
        let mut first = String::new();
        let mut second = String::new();
        let mut focus_request = true;
        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });
        let terminal_ime_event = if key == egui::Key::Enter {
            egui::Event::Ime(egui::ImeEvent::Commit("あ".to_owned()))
        } else {
            egui::Event::Ime(egui::ImeEvent::Disabled)
        };
        let _ = ctx.run(
            egui::RawInput {
                events: vec![
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
                    key_event(key),
                    terminal_ime_event,
                ],
                ..Default::default()
            },
            |ctx| {
                draw_text_fields(
                    ctx,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        );

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
    }
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
