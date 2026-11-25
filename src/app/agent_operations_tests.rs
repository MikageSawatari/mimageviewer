//! Headless App admission, existing operation commit, and readback regressions.
#![cfg(windows)]
use super::*;
use crate::agent_ipc::open::{OpenController, UiOpenOutcome, UiRequestKind};
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::keymap::{KeyAction, Keymap};
use crate::ring_shortcut::{
    MouseGestureBinding, MouseGestureDirection, RingActionId, VideoNormalWheelActionId,
};
use mimageviewer_agent_ipc::{
    AgentLevel, MAX_MCP_RESPONSE_CHARS, OperationEdits, OperationRow, PageRow,
};
use serde_json::{Value, json};

fn fixture(ctx: &egui::Context) -> AppTestEnv {
    let mut app = setup_app();
    app.initialized = true;
    app.indexer_init = IndexerInit::Unavailable;
    app.settings.agent_level = AgentLevel::ReadOnly;
    app.settings.agent_allow_settings = true;
    app.install_agent_changes(crate::agent_ipc::changes::ChangeController::new(), ctx);
    app
}

fn edits(value: Value) -> OperationEdits {
    serde_json::from_value(value).unwrap()
}

fn assert_no_publication(app: &App) {
    let ui = app.agent_changes.as_ref().unwrap();
    assert!(ui.controller.records().lock().unwrap().entries().is_empty());
    assert!(ui.notices.try_recv().is_err());
}

fn assert_final_budget(value: &impl serde::Serialize) {
    assert!(mcp_content_chars(value).unwrap() <= MAX_PAGE_CHARS);
    let structured = serde_json::to_value(value).unwrap();
    let response = json!({"jsonrpc":"2.0", "id":"\0".repeat(256), "result":{
        "content":[{"type":"text","text":structured.to_string()}],"structuredContent":structured,
        "isError":false,"resultType":"complete","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"mimageviewer","version":env!("CARGO_PKG_VERSION")}}
    }});
    assert!(response.to_string().chars().count() <= MAX_MCP_RESPONSE_CHARS);
}

#[test]
fn agent_operations_commit_updates_effective_keys_saves_once_and_publishes_readable_undo() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let before = crate::settings::save_generation();
    let result = app.agent_set_operations(&edits(json!({"keys":{"FsCopyImage":["Ctrl+C"]},"inputs":{"option.gamepad_enabled":false}})), "operations-client").unwrap();
    assert_eq!(crate::settings::save_generation(), before + 1);
    assert_eq!(
        app.keymap
            .effective_chords(KeyAction::FsCopyImage)
            .iter()
            .map(|chord| chord.settings_name())
            .collect::<Vec<_>>(),
        vec!["Ctrl+C"]
    );
    assert_eq!(
        Keymap::from_settings(&crate::settings::Settings::load().keymap)
            .effective_chords(KeyAction::FsCopyImage),
        app.keymap.effective_chords(KeyAction::FsCopyImage)
    );
    assert!(app.pref_state.is_none() && app.operation_customize_state.is_none());
    let records = app.agent_changes.as_ref().unwrap().controller.records();
    let records = records.lock().unwrap();
    assert_eq!(records.entries().len(), 1);
    assert_eq!(records.entries()[0].tool, "set_operations");
    assert_eq!(records.entries()[0].rows.len(), result.changed.len());
    let row = records.entries()[0]
        .rows
        .iter()
        .find(|row| row.item.contains("画像をクリップボードにコピー"))
        .unwrap();
    assert_eq!(row.before, "なし");
    assert_eq!(row.after, "Ctrl+C");
    assert!(
        app.agent_changes
            .as_ref()
            .unwrap()
            .notices
            .try_recv()
            .unwrap()
            .contains("Ctrl+C")
    );
    assert_final_budget(&result);
    drop(records);
    app.agent_set_operations(&edits(json!({"keys":{"FsCopyImage":null}})), "undo")
        .unwrap();
    assert!(
        app.keymap
            .effective_chords(KeyAction::FsCopyImage)
            .is_empty()
    );
}

#[test]
fn agent_operations_normalized_keys_save_canonical_values_and_skip_equivalent_writes() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let canonical = json!(["Ctrl+C", "F13", "F13"]);
    let result = app
        .agent_set_operations(
            &edits(json!({"keys":{"FsCopyImage":[" ctrl + c ", "f13", " F13 "]}})),
            "normalized-first",
        )
        .unwrap();
    assert_eq!(result.changed.len(), 1);
    assert_eq!(result.changed[0].after, canonical);
    let saved = crate::settings::Settings::load();
    let binding = saved
        .keymap
        .overrides
        .iter()
        .find(|binding| binding.action == "FsCopyImage")
        .unwrap();
    assert_eq!(json!(binding.chords), canonical);
    assert_eq!(saved.keymap, app.settings.keymap);
    let expected = app.settings.keymap.clone();
    let generation = crate::settings::save_generation();
    let ui = app.agent_changes.as_ref().unwrap();
    assert_eq!(ui.controller.records().lock().unwrap().entries().len(), 1);
    assert!(ui.notices.try_recv().is_ok());

    let result = app
        .agent_set_operations(
            &edits(json!({"keys":{"FsCopyImage":["CTRL+C", " F13", "f13 "]}})),
            "normalized-same",
        )
        .unwrap();
    assert!(result.changed.is_empty());
    assert_eq!(result.unchanged.len(), 1);
    assert_eq!(result.unchanged[0].before, canonical);
    assert_eq!(result.unchanged[0].after, canonical);
    assert_eq!(app.settings.keymap, expected);
    assert_eq!(crate::settings::save_generation(), generation);
    let ui = app.agent_changes.as_ref().unwrap();
    assert_eq!(ui.controller.records().lock().unwrap().entries().len(), 1);
    assert!(ui.notices.try_recv().is_err());
}

#[test]
fn agent_operations_noop_preserves_override_order_without_saving_or_notifying() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.settings.keymap.overrides = vec![crate::keymap::KeyBindingOverride {
        action: "FsCopyImage".into(),
        chords: vec!["F13".into(), "F13".into()],
    }];
    app.keymap = Keymap::from_settings(&app.settings.keymap);
    let saved = crate::settings::save_generation();
    let before = app.settings.keymap.clone();
    let result = app.agent_set_operations(&edits(json!({"keys":{"FsCopyImage":["F13","F13"]}, "inputs":{"option.gamepad_enabled":true}})), "noop").unwrap();
    assert!(result.changed.is_empty());
    assert_eq!(result.unchanged.len(), 2);
    assert_eq!(app.settings.keymap, before);
    assert_eq!(crate::settings::save_generation(), saved);
    assert_no_publication(&app);
}

#[test]
fn agent_operations_refuse_permission_startup_all_drafts_modals_and_remote_without_changes() {
    let ctx = egui::Context::default();
    for mode in [
        "level",
        "permission",
        "startup",
        "overlay",
        "operation",
        "discard",
        "preferences",
        "draft",
        "restore",
        "migration",
        "remote",
    ] {
        let mut app = fixture(&ctx);
        let session = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(session.clone());
        let expected = match mode {
            "level" => {
                app.settings.agent_level = AgentLevel::Disabled;
                ErrorCode::Disabled
            }
            "permission" => {
                app.settings.agent_allow_settings = false;
                ErrorCode::Disabled
            }
            "startup" => {
                app.initialized = false;
                ErrorCode::NotReady
            }
            "overlay" => {
                app.indexer_init = IndexerInit::NotStarted;
                ErrorCode::NotReady
            }
            "operation" => {
                app.show_operation_customize = true;
                ErrorCode::Busy
            }
            "discard" => {
                app.show_operation_customize_discard_confirm = true;
                ErrorCode::Busy
            }
            "preferences" => {
                app.show_preferences = true;
                ErrorCode::Busy
            }
            "draft" => {
                let settings = app.settings.preferences_snapshot();
                app.install_headless_preferences_state(settings);
                ErrorCode::Busy
            }
            "restore" => {
                app.show_settings_restore = true;
                ErrorCode::Busy
            }
            "migration" => {
                app.show_mouse_nav_migration_prompt = true;
                ErrorCode::Busy
            }
            "remote" => {
                session.acquire(mimageviewer_ipc::SessionAcquireRequest {
                    client_id: "operations-test".into(),
                    peer: mimageviewer_ipc::SessionPeerInfo {
                        connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                        device_name: None,
                    },
                });
                ErrorCode::Busy
            }
            _ => unreachable!(),
        };
        let before = OperationCustomizeBundle::from_settings(&app.settings);
        let generation = crate::settings::save_generation();
        let error = app
            .agent_set_operations(&edits(json!({"keys":{"FsCopyImage":["F13"]}})), "refused")
            .unwrap_err();
        assert_eq!(error.code, expected, "{mode}");
        assert_eq!(crate::settings::save_generation(), generation);
        assert!(before.settings_eq(&OperationCustomizeBundle::from_settings(&app.settings)));
        if matches!(
            mode,
            "level" | "permission" | "startup" | "overlay" | "remote"
        ) {
            assert_eq!(
                app.agent_get_operations(&OperationsQuery::default())
                    .unwrap_err()
                    .code,
                expected
            );
        }
        assert_no_publication(&app);
    }
}

#[test]
fn agent_operations_reads_live_assignments_during_drafts_and_dialog_reopens_with_new_values() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.agent_set_operations(
        &edits(json!({"keys":{"FsCopyImage":["F13"]},"inputs":{"option.gamepad_enabled":false}})),
        "write",
    )
    .unwrap();
    app.show_operation_customize = true;
    let _ = ctx.run_pass(egui::RawInput::default(), |ctx| {
        app.show_operation_customize_dialog(ctx)
    });
    let state = app.operation_customize_state.as_ref().unwrap();
    assert_eq!(state.settings.keymap, app.settings.keymap);
    assert!(!state.settings.gamepad_enabled);
    let page = app
        .agent_get_operations(&serde_json::from_value(json!({"actions":["FsCopyImage"]})).unwrap())
        .unwrap();
    assert!(
        matches!(&page.items[0], PageRow::Value(OperationRow::Key(row)) if row.keys == ["F13"])
    );
}

#[test]
fn agent_operations_unchanged_preferences_ok_preserves_keys_rings_gamepad_and_wheel() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.agent_set_operations(
        &edits(json!({"keys":{"FsCopyImage":["F13"]}, "inputs":{
            "ring.image.up":"image_copy_to_clipboard", "option.gamepad_enabled":false,
            "option.video_normal_wheel_action":"adjust_volume"
        }})),
        "before-preferences",
    )
    .unwrap();
    let expected = OperationCustomizeBundle::from_settings(&app.settings);
    let snapshot = app.settings.preferences_snapshot();
    app.install_headless_preferences_state(snapshot);
    app.apply_preferences_dialog(&ctx);
    assert!(expected.settings_eq(&OperationCustomizeBundle::from_settings(&app.settings)));
    assert_eq!(
        app.settings.ring_shortcuts.video_normal_wheel_action,
        VideoNormalWheelActionId::AdjustVolume
    );
}

#[test]
fn agent_operations_oversized_before_after_and_conflicts_refuse_before_commit() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    // Valid duplicate gesture registrations are unlimited in the editor.
    let large = vec![json!({"pattern":"DR", "action":"none"}); MAX_ITEM_CHARS];
    let before = OperationCustomizeBundle::from_settings(&app.settings);
    let generation = crate::settings::save_generation();
    let error = app
        .agent_set_operations(
            &edits(json!({"keys":{"FsCopyImage":["F13"]}, "inputs":{"gesture.image":large}})),
            "large-after",
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadRequest);
    assert_eq!(generation, crate::settings::save_generation());
    assert!(before.settings_eq(&OperationCustomizeBundle::from_settings(&app.settings)));
    app.settings.ring_shortcuts.mouse_gestures_image.bindings = vec![
        MouseGestureBinding::new(
            vec![MouseGestureDirection::Down, MouseGestureDirection::Right],
            RingActionId::None
        );
        MAX_ITEM_CHARS
    ];
    assert!(
        app.agent_set_operations(
            &edits(json!({"inputs":{"gesture.image":[]}})),
            "large-before"
        )
        .is_err()
    );
    assert_eq!(generation, crate::settings::save_generation());
    assert_no_publication(&app);
    // Also exercise a page-wide response overflow made only of short conflicts.
    app.settings
        .ring_shortcuts
        .mouse_gestures_image
        .bindings
        .clear();
    for &action in KeyAction::all() {
        if action.is_user_facing() && action.trigger() == crate::keymap::KeyTrigger::Press {
            app.settings
                .keymap
                .overrides
                .push(crate::keymap::KeyBindingOverride {
                    action: action.ini_name().into(),
                    chords: vec!["F13".into()],
                });
        }
    }
    let actions: Vec<_> = KeyAction::all()
        .iter()
        .copied()
        .filter(|action| {
            action.is_user_facing() && action.trigger() == crate::keymap::KeyTrigger::Press
        })
        .take(mimageviewer_agent_ipc::MAX_SETTINGS_KEYS)
        .collect();
    let mut keys = serde_json::Map::new();
    for action in actions {
        app.settings.keymap.remove_override(action);
        keys.insert(action.ini_name().into(), json!(["F13"]));
    }
    let before = app.settings.keymap.clone();
    assert!(
        app.agent_set_operations(
            &OperationEdits {
                keys: Some(keys),
                inputs: None
            },
            "large-conflicts"
        )
        .is_err()
    );
    assert_eq!(app.settings.keymap, before);
    assert_eq!(generation, crate::settings::save_generation());
}

#[test]
fn agent_operations_queued_requests_recheck_live_permission_and_use_root_poll_owner() {
    let ctx = egui::Context::default();
    for write in [false, true] {
        for allowed in [false, true] {
            let mut app = fixture(&ctx);
            let controller = OpenController::new(None);
            app.install_agent_open(controller.clone(), &ctx);
            let (ready, queued) = std::sync::mpsc::channel();
            controller.set_enqueue_observer(ready);
            let kind = if write {
                UiRequestKind::SetOperations {
                    args: edits(json!({"keys":{"FsCopyImage":["F13"]}})),
                    client_name: "queued".into(),
                }
            } else {
                UiRequestKind::GetOperations(OperationsQuery::default())
            };
            let worker = std::thread::spawn(move || controller.execute_settings_for_test(kind));
            queued
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            app.settings.agent_allow_settings = allowed;
            app.poll_agent_open_requests(&ctx);
            match (allowed, write, worker.join().unwrap().unwrap()) {
                (false, _, UiOpenOutcome::Refused(error)) => {
                    assert_eq!(error.code, ErrorCode::Disabled)
                }
                (
                    true,
                    true,
                    UiOpenOutcome::Written(Ok(mimageviewer_agent_ipc::Payload::OperationsWrite(
                        result,
                    ))),
                ) => assert_eq!(result.changed.len(), 1),
                (true, false, UiOpenOutcome::Operations(page)) => assert_final_budget(&page),
                _ => panic!("unexpected operations handoff"),
            }
        }
    }
}

#[test]
fn agent_operations_mouse_migration_prompt_commits_live_values_without_a_stale_draft() {
    use egui_kittest::kittest::Queryable;
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.show_mouse_nav_migration_prompt = true;
    let mut harness = crate::test_ui::builder()
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui_state(
            |ui, app| app.show_mouse_nav_migration_prompt_dialog(ui.ctx()),
            app,
        );
    harness.run();
    harness.get_by_label("従来どおり").click();
    harness.run();
    assert!(!harness.state().show_mouse_nav_migration_prompt);
    assert!(
        harness
            .state()
            .settings
            .ring_shortcuts
            .mouse_nav_prompt_done
    );
    assert!(
        harness.state().pref_state.is_none() && harness.state().operation_customize_state.is_none()
    );
    harness
        .state_mut()
        .agent_set_operations(
            &edits(json!({"inputs":{"mouse.image.back":"image_copy_to_clipboard"}})),
            "after-migration",
        )
        .unwrap();
    harness.run();
    assert_eq!(
        harness
            .state()
            .settings
            .ring_shortcuts
            .mouse_buttons_image
            .back,
        RingActionId::ImageCopyToClipboard
    );
    assert!(
        harness
            .state()
            .settings
            .ring_shortcuts
            .mouse_nav_prompt_done
    );
}

#[test]
fn agent_operations_settings_errors_route_owned_fields_to_the_operations_tool() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let generation = crate::settings::save_generation();
    for key in ["keymap", "ring_shortcuts", "gamepad_enabled"] {
        let changes = serde_json::from_value(json!({key:null})).unwrap();
        let error = app
            .agent_set_settings(&changes, "wrong-tool", &ctx)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::BadRequest);
        assert!(error.message.contains(key));
        assert!(error.message.contains("set_operations で変えます"));
    }
    assert_eq!(crate::settings::save_generation(), generation);
    assert_no_publication(&app);
}
