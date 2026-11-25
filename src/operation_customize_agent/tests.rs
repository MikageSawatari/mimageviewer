use super::*;

fn query(value: Value) -> OperationsQuery {
    serde_json::from_value(value).unwrap()
}

fn edits(value: Value) -> OperationEdits {
    serde_json::from_value(value).unwrap()
}

fn key_rows(page: OperationsPage) -> Vec<OperationKeyRow> {
    page.items
        .into_iter()
        .filter_map(|row| match row {
            PageRow::Value(OperationRow::Key(row)) => Some(row),
            PageRow::TooLong(_) => None,
            _ => panic!("wrong view"),
        })
        .collect()
}

fn public_values(bundle: &OperationCustomizeBundle) -> Value {
    json!({
        "keys": KeyAction::all().iter().copied().filter(|action| action.is_user_facing())
            .map(|action| (action.ini_name().to_owned(), key_value(bundle, action))).collect::<serde_json::Map<_, _>>(),
        "inputs": Slot::all().into_iter().map(|slot| (slot.name(), slot.value(bundle))).collect::<serde_json::Map<_, _>>()
    })
}

fn undo(result: &OperationsWriteResult) -> OperationEdits {
    let mut keys = serde_json::Map::new();
    let mut inputs = serde_json::Map::new();
    for row in &result.changed {
        if Slot::parse(&row.name).is_ok() {
            inputs.insert(row.name.clone(), row.before.clone());
        } else {
            keys.insert(row.name.clone(), row.before.clone());
        }
    }
    OperationEdits {
        keys: Some(keys),
        inputs: Some(inputs),
    }
}

#[test]
fn operations_slots_choices_match_ui_validators_and_reject_wrong_addresses() {
    let defaults = OperationCustomizeBundle::defaults();
    let slots = Slot::all();
    let names: HashSet<_> = slots.iter().map(|slot| slot.name()).collect();
    assert_eq!(names.len(), slots.len());
    for slot in slots {
        assert_eq!(Slot::parse(&slot.name()).unwrap().name(), slot.name());
        for choice in slot.choices() {
            let value = if matches!(slot, Slot::Gesture(_)) {
                json!([{"pattern":"DR", "action":choice.value}])
            } else {
                choice.value
            };
            let mut bundle = defaults.clone();
            slot.assign(&mut bundle, &value).unwrap();
            let mut normalized = bundle.clone();
            normalized.ring_shortcuts.sanitize();
            assert_eq!(
                slot.value(&bundle),
                slot.value(&normalized),
                "{}",
                slot.name()
            );
        }
    }
    for name in [
        "Ring.grid.up",
        "ring.edit.up",
        "ring.grid.UP",
        "mouse.edit.back",
        "mouse.image.left",
        "gesture.image.DR",
        "short_right_click.grid",
        "option.mouse_flick_enabled",
        "option.fullscreen_primary_click",
        "ring.grid.up ",
    ] {
        assert!(Slot::parse(name).is_err(), "{name}");
    }
}

#[test]
fn operations_all_slot_kinds_and_duplicate_gestures_round_trip_public_values() {
    let mut before = OperationCustomizeBundle::defaults();
    before.ring_shortcuts.mouse_gestures_image.bindings = vec![
        MouseGestureBinding::new(parse_pattern("DR").unwrap(), RingActionId::None),
        MouseGestureBinding::new(
            parse_pattern("DR").unwrap(),
            RingActionId::ImageCopyToClipboard,
        ),
    ];
    let request = edits(json!({"keys":{"FsCopyImage":["Ctrl+C"]}, "inputs":{
        "ring.image.up":"image_copy_to_clipboard", "mouse.image.middle":"image_copy_to_clipboard",
        "gesture.image":[{"pattern":"UR", "action":"none"}], "right_drag.image":"mouse_gesture",
        "short_right_click.image":"context_menu", "option.gamepad_enabled":false,
        "option.mouse_ring_help_visible":false, "option.mouse_gesture_help_visible":false,
        "option.mouse_gesture_result_toast_visible":false, "option.select_grid_item_on_right_drag_start":true,
        "option.video_normal_wheel_action":"adjust_volume"
    }}));
    let (after, result) = prepare_edits(&before, &request).unwrap();
    assert!(!result.changed.is_empty());
    let (restored, _) = prepare_edits(&after, &undo(&result)).unwrap();
    assert_eq!(public_values(&restored), public_values(&before));
    assert_eq!(
        restored.ring_shortcuts.mouse_gestures_image.bindings,
        before.ring_shortcuts.mouse_gestures_image.bindings
    );
    // The legacy duplicate-pattern behavior remains first-wins.
    assert_eq!(
        restored
            .ring_shortcuts
            .mouse_gestures_image
            .action_for_pattern(&parse_pattern("DR").unwrap()),
        RingActionId::None
    );
}

#[test]
fn operations_every_slot_restores_public_values_and_null_resets_to_its_default() {
    let before = OperationCustomizeBundle::defaults();
    for slot in Slot::all() {
        let old = slot.value(&before);
        let target = if matches!(slot, Slot::Gesture(_)) {
            json!([{"pattern":"LRLR","action":"none"},{"pattern":"LRLR","action":"none"}])
        } else {
            slot.choices()
                .into_iter()
                .map(|choice| choice.value)
                .find(|value| value != &old)
                .unwrap()
        };
        let (after, result) =
            prepare_edits(&before, &edits(json!({"inputs":{slot.name():target}}))).unwrap();
        assert_eq!(result.changed.len(), 1, "{}", slot.name());
        let (restored, _) = prepare_edits(&after, &undo(&result)).unwrap();
        assert_eq!(
            public_values(&restored),
            public_values(&before),
            "{}",
            slot.name()
        );
        let (reset, _) =
            prepare_edits(&after, &edits(json!({"inputs":{slot.name():null}}))).unwrap();
        assert_eq!(
            public_values(&reset),
            public_values(&before),
            "{}",
            slot.name()
        );
    }
}

#[test]
fn operations_gesture_notation_round_trips_and_refuses_normalization() {
    for pattern in ["U", "DR", "URDL", "LRLR"] {
        assert_eq!(pattern_name(&parse_pattern(pattern).unwrap()), pattern);
    }
    for pattern in ["", "UU", "DRRR", "UDLRU", "dr", "D R", "↓→"] {
        assert!(parse_pattern(pattern).is_none(), "{pattern}");
    }
    let before = OperationCustomizeBundle::defaults();
    for input in [
        json!({"gesture.image":[{"pattern":"DD", "action":"none"}]}),
        json!({"gesture.image":[{"pattern":"DR", "action":"none", "extra":true}]}),
        json!({"gesture.image":[{"pattern":"DR", "action":"unknown"}]}),
        json!({"gesture.image":"DR"}),
        json!({"right_drag.edit":"ring_shortcut"}),
        json!({"right_drag.image":"ring"}),
        json!({"short_right_click.image":"close"}),
        json!({"option.gamepad_enabled":1}),
        json!({"option.video_normal_wheel_action":"unknown"}),
        json!({"ring.grid.up":"video_capture"}),
        json!({"mouse.image.back":"open_location_desktop"}),
    ] {
        assert!(
            prepare_edits(
                &before,
                &edits(json!({"keys":{"FsCopyImage":["F13"]}, "inputs":input}))
            )
            .is_err(),
            "{input}"
        );
    }
    let mut legacy = before.clone();
    legacy.ring_shortcuts.mouse_back_forward_action = MouseBackForwardActionId::TreeFolderPrevNext;
    let error = prepare_edits(
        &legacy,
        &edits(json!({"inputs":{"mouse.grid.back":"none"}})),
    )
    .unwrap_err();
    assert!(error.message.contains("mouse.grid.back"));
    assert!(error.message.contains("正規化"));
}

#[test]
fn operations_keys_validate_trigger_shapes_names_and_array_limits() {
    let before = OperationCustomizeBundle::defaults();
    let press = KeyAction::FsCopyImage;
    let modifier = KeyAction::all()
        .iter()
        .copied()
        .find(|a| a.is_user_facing() && a.trigger() == KeyTrigger::ModifierHold)
        .unwrap();
    let hold = KeyAction::all()
        .iter()
        .copied()
        .find(|a| a.is_user_facing() && a.trigger() == KeyTrigger::KeyHold)
        .unwrap();
    for (action, value) in [
        (press, json!(["F13"])),
        (modifier, json!(["Shift"])),
        (hold, json!(["F13"])),
        (press, json!([])),
        (press, Value::Null),
    ] {
        let mut keys = serde_json::Map::new();
        keys.insert(action.ini_name().into(), value);
        prepare_edits(
            &before,
            &OperationEdits {
                keys: Some(keys),
                inputs: None,
            },
        )
        .unwrap();
    }
    for value in [
        json!(["F13", "F14", "F15", "F16"]),
        json!([""]),
        json!(["none"]),
        json!(["Shift"]),
        json!([4]),
        json!("F13"),
    ] {
        assert!(prepare_edits(&before, &edits(json!({"keys":{"FsCopyImage":value}}))).is_err());
    }
    for name in ["fscopyimage", " FsCopyImage", "unknown"] {
        let error = prepare_edits(&before, &edits(json!({"keys":{name:[]}}))).unwrap_err();
        assert!(error.message.contains(name));
        if name.to_lowercase().trim() == "fscopyimage" {
            assert!(error.message.contains("FsCopyImage"));
        }
    }
    for hidden in KeyAction::all().iter().filter(|a| !a.is_user_facing()) {
        assert!(prepare_edits(&before, &edits(json!({"keys":{hidden.ini_name():[]}}))).is_err());
        assert!(get_operations(&before, &query(json!({"actions":[hidden.ini_name()]}))).is_err());
    }
    let error = prepare_edits(
        &before,
        &edits(json!({"keys":{modifier.ini_name():["Ctrl+C"]}})),
    )
    .unwrap_err();
    assert!(error.message.contains(modifier.ini_name()));
}

#[test]
fn operations_duplicate_key_arrays_restore_and_noop_preserves_override_order() {
    let mut before = OperationCustomizeBundle::defaults();
    before.keymap.overrides = vec![
        KeyBindingOverride {
            action: "FsCopyImage".into(),
            chords: vec!["F13".into(), "F13".into()],
        },
        KeyBindingOverride {
            action: "GridCopyImage".into(),
            chords: vec!["F14".into()],
        },
    ];
    let (same, result) = prepare_edits(
        &before,
        &edits(json!({"keys":{"FsCopyImage":["F13","F13"]}})),
    )
    .unwrap();
    assert!(result.changed.is_empty());
    assert_eq!(same.keymap, before.keymap);
    let (after, result) =
        prepare_edits(&before, &edits(json!({"keys":{"FsCopyImage":["F15"]}}))).unwrap();
    assert_eq!(after.keymap.overrides[0].action, "FsCopyImage");
    assert_eq!(after.keymap.overrides[1], before.keymap.overrides[1]);
    let (restored, _) = prepare_edits(&after, &undo(&result)).unwrap();
    assert_eq!(restored.keymap, before.keymap);
    let (reset, result) =
        prepare_edits(&before, &edits(json!({"keys":{"FsCopyImage":null}}))).unwrap();
    let (restored, _) = prepare_edits(&reset, &undo(&result)).unwrap();
    assert_eq!(public_values(&restored), public_values(&before));
}

#[test]
fn operations_right_drag_null_uses_explicit_default_without_legacy_inheritance() {
    for enabled in [false, true] {
        let mut before = OperationCustomizeBundle::defaults();
        before.ring_shortcuts.mouse_flick_enabled = enabled;
        for &context in RightDragContext::all() {
            let slot = Slot::RightDrag(context);
            let (after, result) =
                prepare_edits(&before, &edits(json!({"inputs":{slot.name():null}}))).unwrap();
            assert_eq!(
                slot.value(&after),
                slot.value(&OperationCustomizeBundle::defaults())
            );
            if !result.changed.is_empty() {
                assert_eq!(
                    after.ring_shortcuts.right_drag_mode(context),
                    RightDragMode::Disabled
                );
                let storage = serde_json::to_value(&after.ring_shortcuts).unwrap();
                assert!(storage[format!("right_drag_{}", context_name(context))].is_string());
            } else {
                assert_eq!(after.ring_shortcuts, before.ring_shortcuts);
            }
        }
    }
}

#[test]
fn operations_screens_match_overlap_for_every_action_pair_and_exceptions() {
    for &first in KeyAction::all() {
        for &second in KeyAction::all() {
            let overlap = keymap::OPERATION_SCREENS.iter().any(|(screen, _)| {
                keymap::action_active_on_screen(first, screen)
                    && keymap::action_active_on_screen(second, screen)
            });
            assert_eq!(
                overlap,
                keymap::actions_can_overlap(first, second),
                "{first:?}/{second:?}"
            );
        }
        assert_eq!(
            keymap::action_active_on_screen(first, "page_overview"),
            keymap::page_overview_action_allowed(first)
        );
    }
    assert!(!keymap::action_active_on_screen(
        KeyAction::FsPageOverviewToggle,
        "video_fullscreen"
    ));
    assert!(keymap::action_active_on_screen(
        KeyAction::HelpShowContextShortcuts,
        "erase"
    ));
}

#[test]
fn operations_screen_key_lookup_includes_global_and_fullscreen_common() {
    let mut bundle = OperationCustomizeBundle::defaults();
    bundle.keymap.overrides = vec![
        KeyBindingOverride {
            action: KeyAction::GlobalOpenFolder.ini_name().into(),
            chords: vec!["F13".into()],
        },
        KeyBindingOverride {
            action: KeyAction::FsToggleMetadata.ini_name().into(),
            chords: vec!["F14".into()],
        },
    ];
    assert!(
        key_rows(get_operations(&bundle, &query(json!({"key":"F13", "context":"Grid"}))).unwrap())
            .is_empty()
    );
    for (key, name) in [("F13", "GlobalOpenFolder"), ("F14", "FsToggleMetadata")] {
        let rows = key_rows(
            get_operations(
                &bundle,
                &query(json!({"key":key,"screen":"image_fullscreen"})),
            )
            .unwrap(),
        );
        assert!(rows.iter().any(|row| row.action == name));
    }
    let page = get_operations(&bundle, &query(json!({"key":"Left","screen":"grid"}))).unwrap();
    assert!(!page.reserved.is_empty());
    assert!(
        get_operations(
            &bundle,
            &query(json!({"actions":["GlobalOpenFolder","FsCopyImage"]}))
        )
        .unwrap()
        .note
        .is_some()
    );
    assert!(
        get_operations(&bundle, &query(json!({"actions":["FsCopyImage"]})))
            .unwrap()
            .note
            .is_none()
    );
}

#[test]
fn operations_conflict_preview_replaces_three_keys_and_filters_action_and_candidate() {
    let mut before = OperationCustomizeBundle::defaults();
    before.keymap.overrides = vec![
        KeyBindingOverride {
            action: "FsCopyImage".into(),
            chords: vec!["F13".into(), "F14".into(), "F15".into()],
        },
        KeyBindingOverride {
            action: "FsToggleMetadata".into(),
            chords: vec!["F13".into(), "F16".into()],
        },
    ];
    for key in ["F16", "F13", "Left"] {
        let page = get_operations(
            &before,
            &query(json!({"view":"conflicts", "actions":["FsCopyImage"], "key":key})),
        )
        .unwrap();
        assert!(!page.items.is_empty(), "{key}");
        for item in page.items {
            let PageRow::Value(OperationRow::Conflict(row)) = item else {
                panic!("conflict row");
            };
            assert_eq!(row.key, key);
            assert!(row.action == "FsCopyImage" || row.other == "FsCopyImage");
            if key == "Left" {
                assert_eq!(row.kind, "reserved");
            }
        }
    }
    assert!(
        get_operations(
            &before,
            &query(json!({"view":"conflicts", "actions":["FsCopyImage"], "key":"Shift"}))
        )
        .is_err()
    );
    let (after, result) =
        prepare_edits(&before, &edits(json!({"keys":{"FsCopyImage":["F16"]}}))).unwrap();
    assert!(!result.conflicts.is_empty());
    assert_eq!(
        key_value(&before, KeyAction::FsCopyImage),
        json!(["F13", "F14", "F15"])
    );
    assert_eq!(readback(&before, &after, &result), result);
}

#[test]
fn operations_conflict_kinds_match_editor_and_default_pairs_do_not_warn() {
    let before = OperationCustomizeBundle::defaults();
    assert!(
        get_operations(&before, &query(json!({"view":"conflicts"})))
            .unwrap()
            .items
            .is_empty()
    );
    let (_, hard) = prepare_edits(
        &before,
        &edits(json!({"keys":{"GridCopyImage":["Ctrl+C"]}})),
    )
    .unwrap();
    assert!(hard.conflicts.iter().any(|row| row.kind == "same_context"
        && (row.action == "GridCopyFiles" || row.other == "GridCopyFiles")));
    let (_, reserved) =
        prepare_edits(&before, &edits(json!({"keys":{"GridCopyImage":["Left"]}}))).unwrap();
    assert!(reserved.conflicts.iter().any(|row| row.kind == "reserved"));
    let (_, overlap) = prepare_edits(
        &before,
        &edits(json!({"keys":{"FsCopyImage":["F13"],"FsToggleMetadata":["F13"]}})),
    )
    .unwrap();
    assert!(overlap.conflicts.iter().any(|row| row.kind == "overlap"));
    let hold = KeyAction::all()
        .iter()
        .copied()
        .find(|action| action.is_user_facing() && action.trigger() == KeyTrigger::KeyHold)
        .unwrap();
    let press = KeyAction::all()
        .iter()
        .copied()
        .find(|action| {
            action.is_user_facing()
                && action.trigger() == KeyTrigger::Press
                && keymap::actions_can_overlap(*action, hold)
        })
        .unwrap();
    let (_, mixed) = prepare_edits(
        &before,
        &edits(json!({"keys":{hold.ini_name():["F13"],press.ini_name():["F13"]}})),
    )
    .unwrap();
    assert!(
        mixed
            .conflicts
            .iter()
            .any(|row| row.kind == "trigger_mismatch")
    );
}

#[test]
fn operations_queries_use_and_matching_exact_context_names_and_page_bounds() {
    let bundle = OperationCustomizeBundle::defaults();
    let rows = key_rows(
        get_operations(
            &bundle,
            &query(json!({"query":"画像 コピー", "context":"Grid"})),
        )
        .unwrap(),
    );
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|row| row.context == "Grid"
        && row.label.contains("画像")
        && row.label.contains("コピー")));
    for value in [
        json!({"context":"grid"}),
        json!({"screen":"image"}),
        json!({"key":"none"}),
        json!({"key":""}),
        json!({"page":u32::MAX}),
    ] {
        assert!(
            get_operations(&bundle, &query(value.clone())).is_err(),
            "{value}"
        );
    }
    let empty = get_operations(&bundle, &query(json!({"query":"no-such-operation-843"}))).unwrap();
    assert!(empty.items.is_empty());
    assert_eq!(empty.page, 1);
    assert_eq!(empty.pages, 1);
}

#[test]
fn operations_views_paginate_keys_inputs_conflicts_and_largest_choice_inventory() {
    let mut bundle = OperationCustomizeBundle::defaults();
    for &action in KeyAction::all() {
        if action.is_user_facing() && action.trigger() == KeyTrigger::Press {
            bundle.keymap.overrides.push(KeyBindingOverride {
                action: action.ini_name().into(),
                chords: vec!["F13".into()],
            });
        }
    }
    let mut all_ids = Vec::new();
    let first = get_operations(&bundle, &query(json!({"key":"F13"}))).unwrap();
    assert!(first.pages > 1);
    for page in 1..=first.pages {
        let response = get_operations(&bundle, &query(json!({"key":"F13", "page":page}))).unwrap();
        assert!(response.items.len() <= SETTINGS_PAGE_SIZE);
        assert!(mcp_content_chars(&response).unwrap() <= MAX_PAGE_CHARS);
        for item in response.items {
            all_ids.push(match item {
                PageRow::Value(OperationRow::Key(row)) => wire_digest(&row.action).unwrap(),
                PageRow::TooLong(marker) => marker.id,
                _ => panic!("wrong key row"),
            });
        }
    }
    let expected: Vec<_> = KeyAction::all()
        .iter()
        .filter(|a| a.is_user_facing() && a.trigger() == KeyTrigger::Press)
        .map(|a| wire_digest(a.ini_name()).unwrap())
        .collect();
    assert_eq!(all_ids, expected);
    let defaults = OperationCustomizeBundle::defaults();
    let slot = Slot::all()
        .into_iter()
        .max_by_key(|slot| slot.choices().len())
        .unwrap();
    let count = slot.choices().len();
    let first = get_operations(
        &defaults,
        &query(json!({"view":"choices", "slot":slot.name()})),
    )
    .unwrap();
    let mut found = 0;
    for page in 1..=first.pages {
        let response = get_operations(
            &defaults,
            &query(json!({"view":"choices", "slot":slot.name(), "page":page})),
        )
        .unwrap();
        assert!(mcp_content_chars(&response).unwrap() <= MAX_PAGE_CHARS);
        found += response.items.len();
    }
    assert_eq!(found, count);
    let page = get_operations(
        &defaults,
        &query(json!({"view":"inputs", "slots":"mouse.image"})),
    )
    .unwrap();
    for item in page.items {
        let PageRow::Value(OperationRow::Input(row)) = item else {
            panic!("input row");
        };
        assert!(row.slot.starts_with("mouse.image"));
    }
    let conflicts = get_operations(&bundle, &query(json!({"view":"conflicts"}))).unwrap();
    assert!(conflicts.pages > 1);
    assert!(mcp_content_chars(&conflicts).unwrap() <= MAX_PAGE_CHARS);
}

#[test]
fn operations_single_oversized_gesture_becomes_too_long_without_losing_other_slots() {
    let mut bundle = OperationCustomizeBundle::defaults();
    bundle.ring_shortcuts.mouse_gestures_image.bindings =
        vec![
            MouseGestureBinding::new(parse_pattern("DR").unwrap(), RingActionId::None);
            MAX_ITEM_CHARS
        ];
    let page = get_operations(&bundle, &query(json!({"view":"inputs"}))).unwrap();
    assert_eq!(page.items.len(), Slot::all().len().min(SETTINGS_PAGE_SIZE));
    assert!(
        page.items
            .iter()
            .any(|row| matches!(row, PageRow::TooLong(_)))
    );
    assert!(page.items.iter().any(|row| matches!(row, PageRow::Value(OperationRow::Input(row)) if row.slot == "option.gamepad_enabled")));
    assert!(mcp_content_chars(&page).unwrap() <= MAX_PAGE_CHARS);
}

#[test]
fn operations_long_key_page_keeps_membership_when_aggregate_budget_omits_rows() {
    let bundle = OperationCustomizeBundle::defaults();
    let first = get_operations(&bundle, &OperationsQuery::default()).unwrap();
    let mut rows = Vec::new();
    for page in 1..=first.pages {
        rows.extend(key_rows(
            get_operations(&bundle, &query(json!({"page":page}))).unwrap(),
        ));
    }
    rows.sort_by_key(|row| std::cmp::Reverse(mcp_content_chars(row).unwrap()));
    rows.truncate(SETTINGS_PAGE_SIZE);
    let slots: Vec<_> = Slot::all()
        .into_iter()
        .filter(|slot| matches!(slot, Slot::Ring(..) | Slot::Mouse(..) | Slot::Gesture(..)))
        .map(Slot::name)
        .collect();
    for row in &mut rows {
        row.inputs = slots.clone();
        row.keys = vec!["Ctrl+Shift+Alt+NumpadSubtract".into(); 3];
    }
    assert!(
        rows.iter()
            .all(|row| mcp_content_chars(row).unwrap() <= MAX_ITEM_CHARS)
    );
    let mut page = OperationsPage {
        items: rows
            .iter()
            .cloned()
            .map(OperationRow::Key)
            .map(PageRow::Value)
            .collect(),
        page: 1,
        pages: 1,
        reserved: vec![],
        note: Some("操作は画面ごとに別です".into()),
    };
    assert!(mcp_content_chars(&page).unwrap() > MAX_PAGE_CHARS);
    page.items = bound_page_rows(
        rows.iter().cloned().map(OperationRow::Key).collect(),
        |row| match row {
            OperationRow::Key(row) => row.action.clone(),
            _ => unreachable!(),
        },
        |items| OperationsPage {
            items: items.to_vec(),
            ..page.clone()
        },
    )
    .unwrap();
    assert_eq!(page.items.len(), rows.len());
    assert!(
        page.items
            .iter()
            .any(|item| matches!(item, PageRow::TooLong(_)))
    );
    for (item, expected) in page.items.iter().zip(&rows) {
        match item {
            PageRow::Value(OperationRow::Key(row)) => assert_eq!(row, expected),
            PageRow::TooLong(marker) => {
                assert_eq!(marker.id, wire_digest(&expected.action).unwrap())
            }
            _ => panic!("wrong key row"),
        }
    }
    assert!(mcp_content_chars(&page).unwrap() <= MAX_PAGE_CHARS);
}

#[test]
fn operations_key_inputs_use_the_shared_ring_button_and_gesture_mapping() {
    let mut bundle = OperationCustomizeBundle::defaults();
    bundle
        .ring_shortcuts
        .image
        .slots
        .fill(RingActionId::ImageCopyToClipboard);
    bundle.ring_shortcuts.mouse_buttons_image.middle = RingActionId::ImageCopyToClipboard;
    bundle.ring_shortcuts.mouse_gestures_image.bindings = vec![MouseGestureBinding::new(
        parse_pattern("DR").unwrap(),
        RingActionId::ImageCopyToClipboard,
    )];
    let row =
        key_rows(get_operations(&bundle, &query(json!({"actions":["FsCopyImage"]}))).unwrap())
            .pop()
            .unwrap();
    for &direction in RingDirection::all() {
        assert!(
            row.inputs
                .contains(&Slot::Ring(RingShortcutContext::ImageFullscreen, direction).name())
        );
    }
    assert!(row.inputs.contains(&"mouse.image.middle".into()));
    assert!(row.inputs.contains(&"gesture.image".into()));
    assert!(row.keys.is_empty() && row.default.is_empty());
}
