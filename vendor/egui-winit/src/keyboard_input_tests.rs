//! Port of the complete product keyboard-delivery and physical-hold regression module.

use super::*;
use winit::{
    event::ElementState,
    keyboard::{Key, KeyCode, NamedKey, PhysicalKey},
};

fn key_event(
    key: egui::Key,
    physical_key: egui::Key,
    pressed: bool,
    modifiers: egui::Modifiers,
) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(physical_key),
        pressed,
        repeat: false,
        modifiers,
    }
}

#[cfg(target_os = "windows")]
#[test]
fn delivery_process_presses_and_repeats_append_no_events_or_read_clipboard() {
    for physical in [
        KeyCode::Enter,
        KeyCode::Escape,
        KeyCode::Tab,
        KeyCode::Space,
        KeyCode::KeyA,
        KeyCode::KeyX,
        KeyCode::KeyC,
        KeyCode::KeyV,
        KeyCode::Insert,
        KeyCode::Delete,
    ] {
        for modifiers in [
            egui::Modifiers::NONE,
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            egui::Modifiers::SHIFT,
        ] {
            for repeat in [false, true] {
                let existing = vec![egui::Event::Text("already queued".into())];
                let mut input = egui::RawInput {
                    events: existing.clone(),
                    ..Default::default()
                };
                append_keyboard_input(
                    &mut input,
                    modifiers,
                    &Key::Named(NamedKey::Process),
                    PhysicalKey::Code(physical),
                    Some("IME-handled text"),
                    ElementState::Pressed,
                    repeat,
                    #[cfg(target_os = "windows")]
                    true,
                    #[cfg(target_os = "windows")]
                    &mut HashMap::new(),
                    true,
                    || panic!("Process must not read clipboard"),
                );
                assert_eq!(
                    input.events, existing,
                    "physical={physical:?}, modifiers={modifiers:?}, repeat={repeat}"
                );
            }
        }
    }
}

#[cfg(target_os = "windows")]
#[test]
fn delivery_processed_press_and_corresponding_release_append_no_events() {
    for (physical, expected) in [
        (KeyCode::Enter, egui::Key::Enter),
        (KeyCode::Escape, egui::Key::Escape),
        (KeyCode::Tab, egui::Key::Tab),
        (KeyCode::Space, egui::Key::Space),
        (KeyCode::KeyA, egui::Key::A),
        (KeyCode::KeyV, egui::Key::V),
    ] {
        let modifiers = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
        let mut input = egui::RawInput::default();
        let mut processed = HashMap::new();
        let physical_key = PhysicalKey::Code(physical);
        assert!(!append_keyboard_input(
            &mut input,
            modifiers,
            &Key::Named(NamedKey::Process),
            physical_key,
            Some("processed text"),
            ElementState::Pressed,
            false,
            true,
            &mut processed,
            true,
            || panic!("Process must not read clipboard"),
        ));
        assert_eq!(processed, HashMap::from([(physical_key, KeyHold::Hidden)]));
        append_keyboard_input(
            &mut input,
            modifiers,
            &Key::Character("original logical key".into()),
            physical_key,
            Some("release text"),
            ElementState::Released,
            false,
            #[cfg(target_os = "windows")]
            true,
            #[cfg(target_os = "windows")]
            &mut processed,
            true,
            || panic!("Release must not read clipboard"),
        );
        assert!(input.events.is_empty());
        assert!(processed.is_empty());
        // The release of a physical key with no processed press still flows.
        append_keyboard_input(
            &mut input,
            modifiers,
            &Key::Character("original logical key".into()),
            physical_key,
            Some("release text"),
            ElementState::Released,
            false,
            true,
            &mut processed,
            true,
            || panic!("Release must not read clipboard"),
        );
        assert_eq!(
            input.events,
            vec![key_event(expected, expected, false, modifiers)]
        );
    }
}

#[test]
fn delivery_ordinary_keys_keep_press_release_text_and_egui_repeat_inference() {
    for (logical, physical, expected, text) in [
        (
            Key::Named(NamedKey::Enter),
            KeyCode::Enter,
            egui::Key::Enter,
            "\r",
        ),
        (
            Key::Named(NamedKey::Escape),
            KeyCode::Escape,
            egui::Key::Escape,
            "",
        ),
        (
            Key::Named(NamedKey::Tab),
            KeyCode::Tab,
            egui::Key::Tab,
            "\t",
        ),
        (
            Key::Named(NamedKey::Space),
            KeyCode::Space,
            egui::Key::Space,
            " ",
        ),
        (Key::Character("a".into()), KeyCode::KeyQ, egui::Key::A, "a"),
        (Key::Character("ж".into()), KeyCode::KeyA, egui::Key::A, "ж"),
    ] {
        for state in [ElementState::Pressed, ElementState::Released] {
            for repeat in [false, true] {
                let mut input = egui::RawInput::default();
                append_keyboard_input(
                    &mut input,
                    egui::Modifiers::NONE,
                    &logical,
                    PhysicalKey::Code(physical),
                    Some(text),
                    state,
                    repeat,
                    #[cfg(target_os = "windows")]
                    true,
                    #[cfg(target_os = "windows")]
                    &mut HashMap::new(),
                    true,
                    || panic!("Ordinary key must not read clipboard"),
                );
                let mut expected_events = vec![key_event(
                    expected,
                    key_from_key_code(physical).unwrap(),
                    state == ElementState::Pressed,
                    egui::Modifiers::NONE,
                )];
                if state == ElementState::Pressed
                    && !text.is_empty()
                    && text.chars().all(is_printable_char)
                {
                    expected_events.push(egui::Event::Text(text.into()));
                }
                assert_eq!(
                    input.events, expected_events,
                    "logical={logical:?}, state={state:?}, repeat={repeat}"
                );
            }
        }
    }
    let mut input = egui::RawInput::default();
    append_keyboard_input(
        &mut input,
        egui::Modifiers::NONE,
        &Key::Character("a".into()),
        PhysicalKey::Code(KeyCode::KeyA),
        None,
        ElementState::Pressed,
        false,
        #[cfg(target_os = "windows")]
        true,
        #[cfg(target_os = "windows")]
        &mut HashMap::new(),
        true,
        || None,
    );
    assert_eq!(
        input.events,
        vec![
            key_event(egui::Key::A, egui::Key::A, true, egui::Modifiers::NONE),
            egui::Event::Text("a".into())
        ]
    );
}

#[test]
fn delivery_ordinary_clipboard_commands_keep_fallback_normalization_and_releases() {
    for modifiers in [
        egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
        egui::Modifiers::CTRL | egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Modifiers::CTRL | egui::Modifiers::COMMAND | egui::Modifiers::ALT,
        egui::Modifiers::CTRL
            | egui::Modifiers::COMMAND
            | egui::Modifiers::SHIFT
            | egui::Modifiers::ALT,
    ] {
        for (physical, character, expected) in [
            (KeyCode::KeyX, "x", egui::Event::Cut),
            (KeyCode::KeyC, "c", egui::Event::Copy),
            (
                KeyCode::KeyV,
                "v",
                egui::Event::Paste("first\nsecond".into()),
            ),
        ] {
            for logical in [Key::Character(character.into()), Key::Character("ж".into())] {
                for repeat in [false, true] {
                    let mut input = egui::RawInput::default();
                    let mut reads = 0;
                    append_keyboard_input(
                        &mut input,
                        modifiers,
                        &logical,
                        PhysicalKey::Code(physical),
                        Some(character),
                        ElementState::Pressed,
                        repeat,
                        #[cfg(target_os = "windows")]
                        true,
                        #[cfg(target_os = "windows")]
                        &mut HashMap::new(),
                        true,
                        || {
                            reads += 1;
                            Some(PasteContent::Text("first\r\nsecond".into()))
                        },
                    );
                    assert_eq!(input.events, vec![expected.clone()]);
                    assert_eq!(reads, usize::from(physical == KeyCode::KeyV));
                    input.events.clear();
                    append_keyboard_input(
                        &mut input,
                        modifiers,
                        &logical,
                        PhysicalKey::Code(physical),
                        Some(character),
                        ElementState::Released,
                        false,
                        #[cfg(target_os = "windows")]
                        true,
                        #[cfg(target_os = "windows")]
                        &mut HashMap::new(),
                        true,
                        || panic!("Release must not read clipboard"),
                    );
                    let key = key_from_key_code(physical).unwrap();
                    assert_eq!(input.events, vec![key_event(key, key, false, modifiers)]);
                }
            }
        }
    }
}

#[test]
fn delivery_paste_without_clipboard_contents_appends_no_events() {
    for contents in [None, Some(String::new())] {
        let modifiers = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
        let mut input = egui::RawInput::default();
        let mut reads = 0;
        append_keyboard_input(
            &mut input,
            modifiers,
            &Key::Character("v".into()),
            PhysicalKey::Code(KeyCode::KeyV),
            Some("v"),
            ElementState::Pressed,
            false,
            #[cfg(target_os = "windows")]
            true,
            #[cfg(target_os = "windows")]
            &mut HashMap::new(),
            true,
            || {
                reads += 1;
                contents.map(PasteContent::Text)
            },
        );
        assert_eq!(reads, 1);
        assert!(input.events.is_empty());
    }
}

#[cfg(target_os = "windows")]
#[test]
fn ime_process_press_is_not_forwarded() {
    // The decision does not depend on physical key, text, modifiers, or
    // repeat: it precedes every Key/Text/Cut/Copy/Paste producer.
    let mut processed = HashMap::new();
    let physical = PhysicalKey::Code(KeyCode::Enter);
    assert!(try_on_ime_processed_keyboard_input(
        true,
        &mut processed,
        &Key::Named(NamedKey::Process),
        physical,
        ElementState::Pressed,
    ));
    assert_eq!(processed, HashMap::from([(physical, KeyHold::Hidden)]));
}

#[cfg(target_os = "windows")]
#[test]
fn only_corresponding_processed_physical_releases_are_filtered() {
    let enter = PhysicalKey::Code(KeyCode::Enter);
    let tab = PhysicalKey::Code(KeyCode::Tab);
    let mut processed = HashMap::from([(enter, KeyHold::Hidden)]);
    assert!(!try_on_ime_processed_keyboard_input(
        true,
        &mut processed,
        &Key::Named(NamedKey::Tab),
        tab,
        ElementState::Released,
    ));
    assert_eq!(processed, HashMap::from([(enter, KeyHold::Hidden)]));
    assert!(try_on_ime_processed_keyboard_input(
        true,
        &mut processed,
        &Key::Named(NamedKey::Enter),
        enter,
        ElementState::Released,
    ));
    assert!(processed.is_empty());
    assert!(!try_on_ime_processed_keyboard_input(
        true,
        &mut processed,
        &Key::Named(NamedKey::Enter),
        enter,
        ElementState::Released,
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn disabled_ime_forwards_unrecorded_input_but_removes_recorded_releases() {
    let physical = PhysicalKey::Code(KeyCode::KeyV);
    let mut processed = HashMap::from([(physical, KeyHold::Hidden)]);
    let modifiers = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
    let mut input = egui::RawInput::default();
    assert!(!append_keyboard_input(
        &mut input,
        modifiers,
        &Key::Character("v".into()),
        physical,
        None,
        ElementState::Released,
        false,
        false,
        &mut processed,
        true,
        || panic!("Suppressed release must not read clipboard"),
    ));
    assert!(processed.is_empty());
    assert!(input.events.is_empty());
    let mut reads = 0;
    assert!(append_keyboard_input(
        &mut input,
        modifiers,
        &Key::Named(NamedKey::Process),
        physical,
        Some("v"),
        ElementState::Pressed,
        true,
        false,
        &mut processed,
        true,
        || {
            reads += 1;
            Some(PasteContent::Text("clipboard".into()))
        },
    ));
    assert_eq!(reads, 1);
    assert_eq!(input.events, vec![egui::Event::Paste("clipboard".into())]);
    assert!(append_keyboard_input(
        &mut input,
        modifiers,
        &Key::Character("v".into()),
        physical,
        None,
        ElementState::Released,
        false,
        false,
        &mut processed,
        true,
        || panic!("Release must not read clipboard"),
    ));
    assert_eq!(
        input.events[1],
        key_event(egui::Key::V, egui::Key::V, false, modifiers)
    );
    assert!(processed.is_empty());
}

#[cfg(target_os = "windows")]
#[test]
fn delivery_ime_permission_changes_do_not_leave_enter_down() {
    for release_allowed in [false, true] {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        let mut processed = HashMap::new();
        let physical = PhysicalKey::Code(KeyCode::Enter);
        for (logical, state, allowed, forwarded) in [
            (NamedKey::Process, ElementState::Pressed, true, false),
            (
                NamedKey::Enter,
                ElementState::Released,
                release_allowed,
                false,
            ),
            (NamedKey::Enter, ElementState::Pressed, true, true),
            (NamedKey::Enter, ElementState::Released, true, true),
            (NamedKey::Enter, ElementState::Pressed, true, true),
            (NamedKey::Enter, ElementState::Released, false, true),
        ] {
            assert_eq!(
                append_keyboard_input(
                    &mut input,
                    egui::Modifiers::NONE,
                    &Key::Named(logical),
                    physical,
                    None,
                    state,
                    false,
                    allowed,
                    &mut processed,
                    true,
                    || panic!("Enter does not use clipboard"),
                ),
                forwarded
            );
            ctx.run_pass(input.take(), |ctx| {
                ctx.input(|input| {
                    assert_eq!(
                        input.key_down(egui::Key::Enter),
                        forwarded && state == ElementState::Pressed
                    );
                    assert!(
                        !input
                            .events
                            .iter()
                            .any(|event| matches!(event, egui::Event::Key { repeat: true, .. }))
                    );
                });
            })
            .drop_without_applying_deltas();
            if state == ElementState::Released {
                assert!(processed.is_empty());
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn headless_state(ctx: egui::Context, viewport_id: ViewportId) -> State {
    struct NoDisplay;
    impl HasDisplayHandle for NoDisplay {
        fn display_handle(
            &self,
        ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
            Err(raw_window_handle::HandleError::Unavailable)
        }
    }
    State::new(ctx, viewport_id, &NoDisplay, None, None, None)
}

#[cfg(target_os = "windows")]
#[test]
fn forwarded_hold_survives_process_repeats_until_release_or_focus_loss() {
    for initially_hidden in [false, true] {
        for focus_loss in [false, true] {
            let ctx = egui::Context::default();
            let mut state = headless_state(ctx.clone(), ViewportId::ROOT);
            state.on_focused(true);
            let physical = PhysicalKey::Code(KeyCode::Enter);
            if initially_hidden {
                state.allow_ime = true;
                assert!(!state.on_keyboard_input_parts(
                    &Key::Named(NamedKey::Process),
                    physical,
                    None,
                    ElementState::Pressed,
                    false,
                ));
            }
            for (logical, allowed, repeat, forwarded) in [
                (NamedKey::Enter, true, initially_hidden, true),
                (NamedKey::Process, true, true, false),
                (NamedKey::Enter, false, true, true),
                (NamedKey::Process, true, true, false),
            ] {
                state.allow_ime = allowed;
                assert_eq!(
                    state.on_keyboard_input_parts(
                        &Key::Named(logical),
                        physical,
                        None,
                        ElementState::Pressed,
                        repeat,
                    ),
                    forwarded
                );
                assert_eq!(state.key_holds.get(&physical), Some(&KeyHold::Forwarded));
                ctx.run_pass(state.egui_input.take(), |ctx| {
                    assert!(ctx.input(|input| input.key_down(egui::Key::Enter)));
                })
                .drop_without_applying_deltas();
            }
            state.allow_ime = false;
            if focus_loss {
                state.on_focused(false);
            } else {
                assert!(state.on_keyboard_input_parts(
                    &Key::Named(NamedKey::Enter),
                    physical,
                    None,
                    ElementState::Released,
                    false,
                ));
            }
            assert!(state.key_holds.is_empty());
            ctx.run_pass(state.egui_input.take(), |ctx| {
                assert!(!ctx.input(|input| input.key_down(egui::Key::Enter)));
            })
            .drop_without_applying_deltas();
            state.on_focused(true);
            assert!(state.on_keyboard_input_parts(
                &Key::Named(NamedKey::Enter),
                physical,
                None,
                ElementState::Pressed,
                false,
            ));
            ctx.run_pass(state.egui_input.take(), |ctx| {
                ctx.input(|input| {
                    assert!(input.key_down(egui::Key::Enter));
                    assert!(input.events.iter().any(|event| matches!(
                        event,
                        egui::Event::Key {
                            pressed: true,
                            repeat: false,
                            ..
                        }
                    )));
                });
            })
            .drop_without_applying_deltas();
        }
    }
}

#[cfg(target_os = "windows")]
#[test]
fn all_key_hold_sequences_through_length_six_preserve_egui_key_state() {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Input {
        ProcessPress,
        ProcessRepeat,
        Press,
        Repeat,
        Release,
        FocusLost,
    }
    const INPUTS: [Input; 6] = [
        Input::ProcessPress,
        Input::ProcessRepeat,
        Input::Press,
        Input::Repeat,
        Input::Release,
        Input::FocusLost,
    ];

    // InputState::begin_pass inserts/removes keys on press/release and clears
    // them on WindowFocused(false). Its repeat bit is the pre-insert down state.
    // The real InputState is also exercised by the individual State tests.
    fn apply_egui_events(input: &egui::RawInput, down: &mut bool) -> Option<bool> {
        let mut inferred_repeat = None;
        for event in &input.events {
            match event {
                egui::Event::Key {
                    key: egui::Key::Enter,
                    pressed,
                    ..
                } => {
                    if *pressed {
                        inferred_repeat = Some(std::mem::replace(down, true));
                    } else {
                        *down = false;
                    }
                }
                egui::Event::WindowFocused(false) => *down = false,
                _ => panic!("unexpected event: {event:?}"),
            }
        }
        inferred_repeat
    }

    fn visit(
        state: &mut State,
        down: bool,
        path: &mut [(Input, bool); 6],
        depth: usize,
        count: &mut usize,
    ) {
        let physical = PhysicalKey::Code(KeyCode::Enter);
        for input in INPUTS {
            for allow_ime in [false, true] {
                path[depth] = (input, allow_ime);
                let sequence = &path[..=depth];
                let previous_hold = state.key_holds.get(&physical).copied();
                let previous_focus = state.egui_input.focused;
                let previous_allow = state.allow_ime;
                state.allow_ime = allow_ime;
                state.egui_input.events.clear();
                if input == Input::FocusLost {
                    state.on_focused(false);
                } else {
                    let logical = if matches!(input, Input::ProcessPress | Input::ProcessRepeat) {
                        NamedKey::Process
                    } else {
                        NamedKey::Enter
                    };
                    let element = if input == Input::Release {
                        ElementState::Released
                    } else {
                        ElementState::Pressed
                    };
                    let repeat = matches!(input, Input::ProcessRepeat | Input::Repeat);
                    let forwarded = state.on_keyboard_input_parts(
                        &Key::Named(logical),
                        physical,
                        None,
                        element,
                        repeat,
                    );
                    if input == Input::Release {
                        if previous_hold.is_some() {
                            // A known hold's release is delivered iff egui saw a press.
                            assert_eq!(forwarded, down, "{sequence:?}");
                        } else {
                            // Arbitrary sequences include orphan/duplicate releases.
                            // Preserve the specified legacy forwarding for these too.
                            assert!(forwarded, "{sequence:?}");
                        }
                    } else {
                        assert_eq!(
                            forwarded,
                            !(allow_ime && logical == NamedKey::Process),
                            "{sequence:?}"
                        );
                    }
                    assert_eq!(
                        state.egui_input.events.len(),
                        usize::from(forwarded),
                        "{sequence:?}"
                    );
                    if forwarded {
                        assert!(
                            matches!(state.egui_input.events[0], egui::Event::Key {
                            key: egui::Key::Enter, pressed, ..
                        } if pressed == (element == ElementState::Pressed)),
                            "{sequence:?}"
                        );
                    }
                }
                let mut next_down = down;
                apply_egui_events(&state.egui_input, &mut next_down);
                if matches!(input, Input::Release | Input::FocusLost) {
                    assert!(!next_down, "{sequence:?}");
                    assert!(state.key_holds.is_empty(), "{sequence:?}");
                } else {
                    let expected = if next_down {
                        KeyHold::Forwarded
                    } else {
                        KeyHold::Hidden
                    };
                    assert_eq!(
                        state.key_holds.get(&physical),
                        Some(&expected),
                        "{sequence:?}"
                    );
                }
                if input == Input::Release {
                    // Probe the next fresh press through the same State delivery path.
                    state.egui_input.events.clear();
                    assert!(
                        state.on_keyboard_input_parts(
                            &Key::Named(NamedKey::Enter),
                            physical,
                            None,
                            ElementState::Pressed,
                            false,
                        ),
                        "{sequence:?}"
                    );
                    assert_eq!(
                        apply_egui_events(&state.egui_input, &mut next_down),
                        Some(false),
                        "{sequence:?}"
                    );
                    state.egui_input.events.clear();
                    assert!(
                        state.on_keyboard_input_parts(
                            &Key::Named(NamedKey::Enter),
                            physical,
                            None,
                            ElementState::Released,
                            false,
                        ),
                        "{sequence:?}"
                    );
                    apply_egui_events(&state.egui_input, &mut next_down);
                    assert!(!next_down, "{sequence:?}");
                    assert!(state.key_holds.is_empty(), "{sequence:?}");
                }
                *count += 1;
                if depth + 1 < path.len() {
                    visit(state, next_down, path, depth + 1, count);
                }
                // Restore the parent prefix; no columns are skipped or merged.
                if let Some(hold) = previous_hold {
                    state.key_holds.insert(physical, hold);
                } else {
                    state.key_holds.remove(&physical);
                }
                state.egui_input.focused = previous_focus;
                state.allow_ime = previous_allow;
                state.egui_input.events.clear();
            }
        }
    }

    let mut state = headless_state(egui::Context::default(), ViewportId::ROOT);
    let mut count = 0;
    visit(
        &mut state,
        false,
        &mut [(Input::Press, false); 6],
        0,
        &mut count,
    );
    assert_eq!(
        count,
        (1..=6).map(|length| 12_usize.pow(length)).sum::<usize>()
    );
    assert_eq!(count, 3_257_436);
}

#[cfg(target_os = "windows")]
#[test]
fn forwarded_repeat_after_process_press_keeps_release_and_next_press() {
    for use_state in [false, true] {
        for (ordinary_allowed, ordinary_key) in [
            (false, NamedKey::Enter),
            (true, NamedKey::Enter),
            // allow_ime=false keeps upstream's physical fallback for Process.
            (false, NamedKey::Process),
        ] {
            let ctx = egui::Context::default();
            let mut state = headless_state(ctx.clone(), ViewportId::ROOT);
            state.on_focused(true);
            let mut raw_input = egui::RawInput::default();
            let mut processed = HashMap::new();
            let physical = PhysicalKey::Code(KeyCode::Enter);
            for (logical, element, allowed, repeat, forwarded, egui_repeat) in [
                (
                    NamedKey::Process,
                    ElementState::Pressed,
                    true,
                    false,
                    false,
                    false,
                ),
                (
                    ordinary_key,
                    ElementState::Pressed,
                    ordinary_allowed,
                    true,
                    true,
                    false,
                ),
                (
                    ordinary_key,
                    ElementState::Pressed,
                    ordinary_allowed,
                    true,
                    true,
                    true,
                ),
                (
                    NamedKey::Enter,
                    ElementState::Released,
                    ordinary_allowed,
                    false,
                    true,
                    false,
                ),
                (
                    NamedKey::Enter,
                    ElementState::Pressed,
                    true,
                    false,
                    true,
                    false,
                ),
                (
                    NamedKey::Enter,
                    ElementState::Released,
                    true,
                    false,
                    true,
                    false,
                ),
            ] {
                let delivered = if use_state {
                    state.allow_ime = allowed;
                    state.on_keyboard_input_parts(
                        &Key::Named(logical),
                        physical,
                        None,
                        element,
                        repeat,
                    )
                } else {
                    append_keyboard_input(
                        &mut raw_input,
                        egui::Modifiers::NONE,
                        &Key::Named(logical),
                        physical,
                        None,
                        element,
                        repeat,
                        allowed,
                        &mut processed,
                        true,
                        || panic!("Enter must not read clipboard"),
                    )
                };
                assert_eq!(delivered, forwarded);
                let tracked = if use_state {
                    &state.key_holds
                } else {
                    &processed
                };
                let expected = if element == ElementState::Released {
                    None
                } else if forwarded {
                    Some(&KeyHold::Forwarded)
                } else {
                    Some(&KeyHold::Hidden)
                };
                assert_eq!(tracked.get(&physical), expected);
                let input = if use_state {
                    state.egui_input.take()
                } else {
                    raw_input.take()
                };
                ctx.run_pass(input, |ctx| {
                    ctx.input(|input| {
                        assert_eq!(
                            input.key_down(egui::Key::Enter),
                            forwarded && element == ElementState::Pressed
                        );
                        let keys: Vec<_> = input
                            .events
                            .iter()
                            .filter_map(|event| {
                                if let egui::Event::Key {
                                    key,
                                    pressed,
                                    repeat,
                                    ..
                                } = event
                                {
                                    Some((*key, *pressed, *repeat))
                                } else {
                                    None
                                }
                            })
                            .collect();
                        assert_eq!(
                            keys,
                            if forwarded {
                                vec![(
                                    egui::Key::Enter,
                                    element == ElementState::Pressed,
                                    egui_repeat,
                                )]
                            } else {
                                vec![]
                            }
                        );
                    });
                })
                .drop_without_applying_deltas();
            }
        }
    }
}

#[cfg(target_os = "windows")]
#[test]
fn state_delivery_tracks_releases_across_ime_permission_changes() {
    for release_allowed in [false, true] {
        let ctx = egui::Context::default();
        let mut state = headless_state(ctx.clone(), ViewportId::ROOT);
        state.on_focused(true);
        for (logical, element, allowed, forwarded) in [
            (NamedKey::Process, ElementState::Pressed, true, false),
            (
                NamedKey::Enter,
                ElementState::Released,
                release_allowed,
                false,
            ),
            (NamedKey::Enter, ElementState::Pressed, true, true),
            (NamedKey::Enter, ElementState::Released, true, true),
            (NamedKey::Enter, ElementState::Pressed, true, true),
            (NamedKey::Enter, ElementState::Released, false, true),
        ] {
            state.allow_ime = allowed;
            assert_eq!(
                state.on_keyboard_input_parts(
                    &Key::Named(logical),
                    PhysicalKey::Code(KeyCode::Enter),
                    None,
                    element,
                    false
                ),
                forwarded
            );
            ctx.run_pass(state.egui_input.take(), |ctx| {
                ctx.input(|input| {
                    assert_eq!(
                        input.key_down(egui::Key::Enter),
                        forwarded && element == ElementState::Pressed
                    );
                    assert!(
                        !input
                            .events
                            .iter()
                            .any(|event| matches!(event, egui::Event::Key { repeat: true, .. }))
                    );
                });
            })
            .drop_without_applying_deltas();
            if element == ElementState::Released {
                assert!(state.key_holds.is_empty());
            }
        }
    }
}

#[cfg(target_os = "windows")]
#[test]
fn state_focus_loss_clears_only_its_own_processed_keys() {
    for allowed in [false, true] {
        let ctx = egui::Context::default();
        let mut owner = headless_state(ctx.clone(), ViewportId::ROOT);
        let mut sibling = headless_state(ctx.clone(), ViewportId::from_hash_of("sibling"));
        let physical = PhysicalKey::Code(KeyCode::Enter);
        for state in [&mut owner, &mut sibling] {
            state.on_focused(true);
            state.allow_ime = true;
            assert!(!state.on_keyboard_input_parts(
                &Key::Named(NamedKey::Process),
                physical,
                None,
                ElementState::Pressed,
                false
            ));
        }
        owner.allow_ime = allowed;
        owner.on_focused(false);
        assert!(!owner.egui_input.focused);
        assert!(
            owner
                .egui_input
                .events
                .contains(&egui::Event::WindowFocused(false))
        );
        assert!(owner.key_holds.is_empty());
        assert_eq!(
            sibling.key_holds,
            HashMap::from([(physical, KeyHold::Hidden)])
        );
        // The old release goes to another window. After refocusing, the next ordinary
        // press/release must both reach egui, including when allow_ime changes.
        ctx.run_pass(owner.egui_input.take(), |_| {})
            .drop_without_applying_deltas();
        owner.on_focused(true);
        owner.allow_ime = true;
        for element in [
            ElementState::Pressed,
            ElementState::Released,
            ElementState::Pressed,
            ElementState::Released,
        ] {
            assert!(owner.on_keyboard_input_parts(
                &Key::Named(NamedKey::Enter),
                physical,
                None,
                element,
                false
            ));
            ctx.run_pass(owner.egui_input.take(), |ctx| {
                ctx.input(|input| {
                    assert_eq!(
                        input.key_down(egui::Key::Enter),
                        element == ElementState::Pressed
                    );
                    assert!(
                        !input
                            .events
                            .iter()
                            .any(|event| matches!(event, egui::Event::Key { repeat: true, .. }))
                    );
                });
            })
            .drop_without_applying_deltas();
        }
        sibling.allow_ime = false;
        assert!(!sibling.on_keyboard_input_parts(
            &Key::Named(NamedKey::Enter),
            physical,
            None,
            ElementState::Released,
            false
        ));
        assert!(sibling.key_holds.is_empty());
    }
}

#[cfg(target_os = "windows")]
#[test]
fn processed_keys_track_repeats_and_multiple_physical_keys_per_state() {
    let enter = PhysicalKey::Code(KeyCode::Enter);
    let tab = PhysicalKey::Code(KeyCode::Tab);
    let mut owner = HashMap::new();
    let mut sibling = HashMap::new();
    for physical in [enter, enter, tab] {
        assert!(try_on_ime_processed_keyboard_input(
            true,
            &mut owner,
            &Key::Named(NamedKey::Process),
            physical,
            ElementState::Pressed,
        ));
    }
    assert_eq!(
        owner,
        HashMap::from([(enter, KeyHold::Hidden), (tab, KeyHold::Hidden)])
    );
    assert!(!try_on_ime_processed_keyboard_input(
        true,
        &mut sibling,
        &Key::Named(NamedKey::Enter),
        enter,
        ElementState::Released,
    ));
    assert_eq!(
        owner,
        HashMap::from([(enter, KeyHold::Hidden), (tab, KeyHold::Hidden)])
    );
    assert!(sibling.is_empty());
    assert!(try_on_ime_processed_keyboard_input(
        true,
        &mut owner,
        &Key::Named(NamedKey::Enter),
        enter,
        ElementState::Released,
    ));
    assert_eq!(owner, HashMap::from([(tab, KeyHold::Hidden)]));
}

#[cfg(target_os = "windows")]
#[test]
fn ordinary_named_keys_are_forwarded() {
    let mut processed = HashMap::new();
    for key in [
        NamedKey::Enter,
        NamedKey::Escape,
        NamedKey::Tab,
        NamedKey::Space,
    ] {
        assert!(!try_on_ime_processed_keyboard_input(
            true,
            &mut processed,
            &Key::Named(key),
            PhysicalKey::Code(KeyCode::Enter),
            ElementState::Pressed
        ));
        assert!(key_from_winit_key(&Key::Named(key)).is_some());
    }
    assert_eq!(
        processed,
        HashMap::from([(PhysicalKey::Code(KeyCode::Enter), KeyHold::Forwarded)])
    );
}

#[test]
fn non_latin_physical_fallback_keeps_clipboard_commands() {
    let logical = Key::Character("ж".into());
    assert_eq!(key_from_winit_key(&logical), None);
    for (physical, expected) in [
        (KeyCode::KeyX, egui::Key::X),
        (KeyCode::KeyC, egui::Key::C),
        (KeyCode::KeyV, egui::Key::V),
    ] {
        assert_eq!(
            key_from_winit_key(&logical).or(key_from_key_code(physical)),
            Some(expected)
        );
    }
    let modifiers = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
    for (key, expected) in [
        (egui::Key::X, egui_ime::ClipboardCommand::Cut),
        (egui::Key::C, egui_ime::ClipboardCommand::Copy),
        (egui::Key::V, egui_ime::ClipboardCommand::Paste),
    ] {
        assert_eq!(
            egui_ime::winit_clipboard_command(modifiers, key),
            Some(expected)
        );
    }
}
