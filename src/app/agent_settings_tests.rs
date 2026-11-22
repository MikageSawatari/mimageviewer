//! Headless settings requests through the real App admission and Preferences OK.
#![cfg(windows)]
use super::*;
use crate::agent_ipc::changes::{ChangeRowOutcome, ChangeRowResult};
use crate::agent_ipc::open::{OpenController, UiOpenOutcome, UiRequestKind};
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::settings::{
    FavoriteViewState, FullscreenWheelAction, ReadingFlow, Settings, SpreadMode,
};
use serde_json::json;
use std::collections::HashSet;
use std::sync::mpsc;
use std::time::Duration;

fn fixture(ctx: &egui::Context) -> AppTestEnv {
    let mut app = setup_app();
    app.initialized = true;
    app.indexer_init = IndexerInit::Unavailable;
    app.settings.agent_level = AgentLevel::ReadOnly;
    app.settings.agent_allow_settings = true;
    app.install_agent_changes(crate::agent_ipc::changes::ChangeController::new(), ctx);
    app
}

fn changes(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn assert_final_budget(value: &impl serde::Serialize) {
    assert!(mcp_content_chars(value).unwrap() <= MAX_PAGE_CHARS);
    let structured = serde_json::to_value(value).unwrap();
    let text = serde_json::to_string(&structured).unwrap();
    let envelope = json!({"jsonrpc":"2.0", "id":"\0".repeat(256), "result":{
        "content":[{"type":"text","text":text}], "structuredContent":structured, "isError":false,
        "resultType":"complete", "_meta":{"io.modelcontextprotocol/serverInfo":{
            "name":"mimageviewer", "version":env!("CARGO_PKG_VERSION")
        }}
    }});
    assert!(
        serde_json::to_string(&envelope).unwrap().chars().count()
            <= mimageviewer_agent_ipc::MAX_MCP_RESPONSE_CHARS
    );
}

#[test]
fn agent_settings_precommit_bound_covers_mixed_retained_values_and_escaped_rows() {
    let make = |index: usize, before: Value, after: Value| SettingChange {
        key: format!("key{index}"),
        label: format!("label{index}"),
        before: before.clone(),
        after: after.clone(),
        overridden_by_favorite: false,
        rows: vec![SettingChangeRow {
            item: format!("label{index}"),
            before: before.to_string(),
            after: after.to_string(),
        }],
    };
    let mut preview = SettingsWriteResult {
        changed: Vec::new(),
        unchanged: Vec::new(),
    };
    let mut unchanged = preview.clone();
    for index in 0..8 {
        let long = json!(format!("{}\"\0\\", "値".repeat(32)));
        let (before, after) = if index % 2 == 0 {
            (long, json!("x"))
        } else {
            (json!("x"), long)
        };
        preview.changed.push(make(index, before.clone(), after));
        unchanged
            .unchanged
            .push(make(index, before.clone(), before));
    }
    let bound = settings_budget_bound(&preview, &unchanged);
    assert_final_budget(&bound);
    let max = mcp_content_chars(&bound).unwrap();
    for mask in 0..256 {
        let mut actual = SettingsWriteResult {
            changed: Vec::new(),
            unchanged: Vec::new(),
        };
        for index in 0..8 {
            if mask & (1 << index) == 0 {
                actual.changed.push(preview.changed[index].clone());
            } else {
                actual.unchanged.push(unchanged.unchanged[index].clone());
            }
        }
        assert!(mcp_content_chars(&actual).unwrap() <= max);
        assert_final_budget(&actual);
    }
}

fn detail(app: &App, key: &str) -> SettingRow {
    let page = app
        .agent_get_settings(None, Some(&[key.to_owned()]), None)
        .unwrap();
    assert_eq!(page.items.len(), 1);
    match page.items.into_iter().next().unwrap() {
        PageRow::Value(row) => row,
        PageRow::TooLong(_) => panic!("expected a complete detail row for {key}"),
    }
}

fn assert_no_publication(app: &App) {
    let changes = app.agent_changes.as_ref().unwrap();
    assert!(
        changes
            .controller
            .records()
            .lock()
            .unwrap()
            .entries()
            .is_empty()
    );
    assert!(matches!(
        changes.notices.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

fn acquire_remote(session: &crate::remote_ipc::session::SessionHandle) {
    session.acquire(mimageviewer_ipc::SessionAcquireRequest {
        client_id: "settings-test".into(),
        peer: mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: None,
        },
    });
}

#[test]
fn agent_settings_typed_noop_does_not_save_install_a_draft_or_publish() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    // A persisted sentinel distinguishes skipping commit from saving equal values.
    app.settings.video_seek_thumbnail_tolerance_secs = 2.0;
    assert!(app.settings.save_checked());
    app.settings.video_seek_thumbnail_tolerance_secs = 1.0;
    let saved_generation = crate::settings::save_generation();
    let before_toast = app.fs_feedback_toast.as_ref().map(|toast| toast.0.clone());
    for raw in [json!(1), json!(1.0)] {
        let result = app
            .agent_set_settings(
                &changes(json!({"video_seek_thumbnail_tolerance_secs": raw})),
                "numeric-noop",
                &ctx,
            )
            .unwrap();
        assert!(result.changed.is_empty());
        assert_eq!(result.unchanged.len(), 1);
        assert_eq!(result.unchanged[0].before, json!(1.0));
        assert_eq!(result.unchanged[0].after, json!(1.0));
        assert!(app.pref_state.is_none());
        assert!(!app.show_preferences);
        assert_eq!(app.settings.video_seek_thumbnail_tolerance_secs, 1.0);
        assert_eq!(Settings::load().video_seek_thumbnail_tolerance_secs, 2.0);
        assert_eq!(crate::settings::save_generation(), saved_generation);
        assert_eq!(
            app.fs_feedback_toast.as_ref().map(|toast| toast.0.clone()),
            before_toast
        );
        assert_no_publication(&app);
    }
}

#[test]
fn agent_settings_real_commit_updates_memory_database_and_change_record() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.settings.fullscreen_wheel_action = FullscreenWheelAction::PageTurn;
    assert!(app.settings.save_checked());
    let result = app
        .agent_set_settings(
            &changes(json!({"fullscreen_wheel_action": "InPageScroll"})),
            "wheel-client",
            &ctx,
        )
        .unwrap();
    assert_eq!(
        app.settings.fullscreen_wheel_action,
        FullscreenWheelAction::InPageScroll
    );
    assert_eq!(
        Settings::load().fullscreen_wheel_action,
        FullscreenWheelAction::InPageScroll
    );
    assert!(app.pref_state.is_none());
    assert!(!app.show_preferences);
    assert!(result.unchanged.is_empty());
    assert_eq!(result.changed.len(), 1);
    let change = &result.changed[0];
    assert_eq!(change.key, "fullscreen_wheel_action");
    assert_eq!(change.before, json!("PageTurn"));
    assert_eq!(change.after, json!("InPageScroll"));
    assert!(!change.overridden_by_favorite);
    assert!(!change.rows.is_empty());
    let ui = app.agent_changes.as_ref().unwrap();
    let records = ui.controller.records();
    let records = records.lock().unwrap();
    assert_eq!(records.entries().len(), 1);
    let record = &records.entries()[0];
    assert_eq!(record.client_name, "wheel-client");
    assert_eq!(record.tool, "set_settings");
    assert_eq!(record.counts.changed, 1);
    assert_eq!(record.rows.len(), change.rows.len());
    for (record, response) in record.rows.iter().zip(&change.rows) {
        assert_eq!(record.item, response.item);
        assert_eq!(record.before, response.before);
        assert_eq!(record.after, response.after);
        assert_eq!(
            record.result,
            ChangeRowResult::Done(ChangeRowOutcome::Changed)
        );
    }
    assert_eq!(ui.notices.try_recv().unwrap(), record.notice);
    assert_final_budget(&result);
}

#[test]
fn agent_settings_favorite_overlay_reads_and_commits_only_standard_values() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.settings.default_spread_mode = SpreadMode::Single;
    app.settings.default_reading_flow = ReadingFlow::Paged;
    let mut favorite = FavoriteViewState::from_settings(&app.settings);
    favorite.default_spread_mode = SpreadMode::RtlCover;
    favorite.default_reading_flow = ReadingFlow::Horizontal;
    let id = uuid::Uuid::new_v4();
    app.favorite_view_states.insert(id, favorite.clone());
    app.settings.apply_favorite_view_overlay(id, &favorite);
    let row = detail(&app, "default_spread_mode");
    assert_eq!(row.value, json!("Single"));
    assert!(row.overridden_by_favorite);
    let result = app
        .agent_set_settings(
            &changes(json!({"default_spread_mode": "Ltr", "default_reading_flow": "Vertical"})),
            "standard-client",
            &ctx,
        )
        .unwrap();
    assert_eq!(result.changed.len(), 2);
    for (key, before, after) in [
        ("default_spread_mode", "Single", "Ltr"),
        ("default_reading_flow", "Paged", "Vertical"),
    ] {
        let row = result.changed.iter().find(|row| row.key == key).unwrap();
        assert_eq!(row.before, json!(before));
        assert_eq!(row.after, json!(after));
        assert!(row.overridden_by_favorite);
        assert_eq!(detail(&app, key).value, json!(after));
    }
    assert_eq!(FavoriteViewState::from_settings(&app.settings), favorite);
    assert_eq!(app.settings.active_favorite_view_id(), Some(id));
    assert_eq!(app.favorite_view_states.get(&id), Some(&favorite));
    let loaded = Settings::load();
    assert_eq!(loaded.default_spread_mode, SpreadMode::Ltr);
    assert_eq!(loaded.default_reading_flow, ReadingFlow::Vertical);
    assert!(app.pref_state.is_none());
}

#[test]
fn agent_settings_permission_startup_modal_and_hidden_draft_refuse_without_mutation() {
    let ctx = egui::Context::default();
    for mode in [
        "level",
        "permission",
        "startup",
        "overlay",
        "modal",
        "preferences",
        "draft",
        "remote",
    ] {
        let mut app = fixture(&ctx);
        app.settings.fullscreen_wheel_action = FullscreenWheelAction::PageTurn;
        assert!(app.settings.save_checked());
        let session = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(session.clone());
        let mut draft_pointer = None;
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
            "modal" => {
                app.show_about_dialog = true;
                ErrorCode::Busy
            }
            "preferences" => {
                app.show_preferences = true;
                ErrorCode::Busy
            }
            "draft" => {
                let mut candidate = app.settings.preferences_snapshot();
                candidate.fullscreen_wheel_action = FullscreenWheelAction::Zoom;
                app.install_headless_preferences_state(candidate);
                draft_pointer = Some(std::ptr::from_ref(
                    app.pref_state.as_ref().unwrap().as_ref(),
                ));
                ErrorCode::Busy
            }
            "remote" => {
                acquire_remote(&session);
                ErrorCode::Busy
            }
            _ => unreachable!(),
        };
        let error = app
            .agent_set_settings(
                &changes(json!({"fullscreen_wheel_action": "InPageScroll"})),
                "refused",
                &ctx,
            )
            .unwrap_err();
        assert_eq!(error.code, expected, "{mode}");
        assert_eq!(
            app.settings.fullscreen_wheel_action,
            FullscreenWheelAction::PageTurn,
            "{mode}"
        );
        assert_eq!(
            Settings::load().fullscreen_wheel_action,
            FullscreenWheelAction::PageTurn,
            "{mode}"
        );
        if let Some(pointer) = draft_pointer {
            let draft = app.pref_state.as_ref().unwrap();
            assert_eq!(std::ptr::from_ref(draft.as_ref()), pointer);
            assert_eq!(
                draft.settings.fullscreen_wheel_action,
                FullscreenWheelAction::Zoom
            );
            assert!(!app.show_preferences);
        } else {
            assert!(app.pref_state.is_none(), "{mode}");
        }
        assert_no_publication(&app);
    }
}

#[test]
fn agent_settings_read_during_local_draft_returns_live_standard_values() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.settings.fullscreen_wheel_action = FullscreenWheelAction::PageTurn;
    let mut candidate = app.settings.preferences_snapshot();
    candidate.fullscreen_wheel_action = FullscreenWheelAction::Zoom;
    app.install_headless_preferences_state(candidate);
    app.show_preferences = true;
    assert_eq!(
        detail(&app, "fullscreen_wheel_action").value,
        json!("PageTurn")
    );
    assert_eq!(
        app.pref_state
            .as_ref()
            .unwrap()
            .settings
            .fullscreen_wheel_action,
        FullscreenWheelAction::Zoom
    );
    assert!(app.show_preferences);
    assert_no_publication(&app);
}

#[test]
fn agent_settings_search_history_off_clears_live_history_and_publishes_readable_rows() {
    use crate::ui_pinned_search::PinnedSearchKind;
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.settings.save_search_input_history = true;
    for kind in PinnedSearchKind::ALL {
        app.settings
            .search_input_history
            .record(kind, "history-sentinel");
    }
    assert!(app.settings.save_checked());
    let result = app
        .agent_set_settings(
            &changes(json!({"save_search_input_history": false})),
            "history-client",
            &ctx,
        )
        .unwrap();
    assert!(!app.settings.save_search_input_history);
    assert_eq!(app.settings.search_input_history.len(), 0);
    let loaded = Settings::load();
    assert!(!loaded.save_search_input_history);
    assert_eq!(loaded.search_input_history.len(), 0);
    let change = &result.changed[0];
    assert_eq!(change.key, "save_search_input_history");
    assert_eq!(change.before, json!(true));
    assert_eq!(change.after, json!(false));
    assert!(!change.rows.is_empty());
    for row in &change.rows {
        assert!(!row.item.is_empty());
        assert!(!row.before.is_empty());
        assert!(!row.after.is_empty());
        assert_ne!(row.before, "true");
        assert_ne!(row.after, "false");
    }
    let ui = app.agent_changes.as_ref().unwrap();
    let records = ui.controller.records();
    let records = records.lock().unwrap();
    assert_eq!(records.entries().len(), 1);
    assert_eq!(records.entries()[0].rows.len(), change.rows.len());
    for (record, row) in records.entries()[0].rows.iter().zip(&change.rows) {
        assert_eq!(record.item, row.item);
        assert_eq!(record.before, row.before);
        assert_eq!(record.after, row.after);
    }
    let notice = ui.notices.try_recv().unwrap();
    assert!(notice.contains(&change.rows[0].item));
    assert!(notice.contains(&change.rows[0].before));
    assert!(notice.contains(&change.rows[0].after));
    assert!(app.pref_state.is_none());
}

#[test]
fn agent_settings_invalid_and_excluded_keys_refuse_the_whole_request() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.settings.fullscreen_wheel_action = FullscreenWheelAction::PageTurn;
    assert!(app.settings.save_checked());
    for (key, value) in [
        ("agent_level", json!("Organize")),
        ("agent_allow_settings", json!(true)),
        ("remote_service_enabled", json!(true)),
        ("keymap", json!({})),
        ("not_a_setting", json!(true)),
        ("slideshow_interval_secs", json!(0.01)),
    ] {
        let mut request = changes(json!({"fullscreen_wheel_action": "InPageScroll"}));
        request.insert(key.into(), value);
        let error = app
            .agent_set_settings(&request, "invalid-client", &ctx)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::BadRequest, "{key}");
        assert!(error.message.contains(key), "{key}: {}", error.message);
        if key != "slideshow_interval_secs" {
            let error = app
                .agent_get_settings(
                    None,
                    Some(&["fullscreen_wheel_action".into(), key.into()]),
                    None,
                )
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::BadRequest);
            assert!(error.message.contains(key));
        }
        assert_eq!(
            app.settings.fullscreen_wheel_action,
            FullscreenWheelAction::PageTurn
        );
        assert_eq!(
            Settings::load().fullscreen_wheel_action,
            FullscreenWheelAction::PageTurn
        );
        assert!(app.pref_state.is_none());
        assert_no_publication(&app);
    }
}

#[test]
fn agent_settings_oversized_external_tool_response_is_refused_before_commit() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    app.settings.external_tools.clear();
    assert!(app.settings.save_checked());
    let mut tool = crate::external_tool::ExternalTool::defaults_for_viewing();
    tool.name = "large-tool".into();
    tool.arguments = "\\\"\n".repeat(MAX_PAGE_CHARS);
    let request = changes(json!({"external_tools": [tool]}));
    // Prove that the refusal comes from the response preflight, not validation.
    crate::settings_transfer::stage1_validate(&app.settings.preferences_snapshot(), &request)
        .unwrap();
    let error = app
        .agent_set_settings(&request, "large-client", &ctx)
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadRequest);
    assert_eq!(error.message, RESPONSE_TOO_LARGE);
    assert!(app.settings.external_tools.is_empty());
    assert!(Settings::load().external_tools.is_empty());
    assert!(app.pref_state.is_none());
    assert!(!app.show_preferences);
    assert_no_publication(&app);
}

#[test]
fn agent_settings_association_deletion_explains_restore_limit_in_response_record_and_notice() {
    use crate::external_tool::{ExternalTool, ExternalToolLaunch};
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let mut tool = ExternalTool::defaults_for_viewing();
    tool.name = "関連付けテスト".into();
    tool.launch = ExternalToolLaunch::Association {
        handler_id: "existing-handler".into(),
    };
    app.settings.external_tools = vec![tool];
    assert!(app.settings.save_checked());
    let result = app
        .agent_set_settings(
            &changes(json!({"external_tools":[]})),
            "association-delete",
            &ctx,
        )
        .unwrap();
    assert_eq!(result.changed.len(), 1);
    let change = &result.changed[0];
    assert_eq!(change.rows.len(), 1);
    let message = &change.rows[0].after;
    assert!(message.contains("AI 側からは復元できません"));
    assert!(message.contains("環境設定の画面で登録し直して"));
    let ui = app.agent_changes.as_ref().unwrap();
    let records = ui.controller.records();
    assert_eq!(records.lock().unwrap().entries()[0].rows[0].after, *message);
    assert!(ui.notices.try_recv().unwrap().contains(message));
    assert!(app.settings.external_tools.is_empty());
    assert!(Settings::load().external_tools.is_empty());
    let restore = changes(json!({"external_tools":change.before}));
    assert_eq!(
        app.agent_set_settings(&restore, "association-restore", &ctx)
            .unwrap_err()
            .code,
        ErrorCode::BadRequest
    );
    assert!(app.settings.external_tools.is_empty());
    assert_final_budget(&result);
}

#[test]
fn agent_settings_vst_paths_commit_preserves_runtime_state_and_hides_it_from_wire() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    // Keep processing disabled: this headless write never starts a plugin worker.
    app.settings.vst3_enabled = false;
    let existing = app
        .tmp
        .path()
        .join("existing.vst3")
        .to_string_lossy()
        .into_owned();
    let added = app
        .tmp
        .path()
        .join("added.vst3")
        .to_string_lossy()
        .into_owned();
    app.settings.vst3_plugins = vec![crate::settings::Vst3PluginEntry {
        path: existing.clone(),
        state: Some("PRIVATE_RUNNING_VST_STATE".into()),
        bypass: true,
        user_hidden: true,
        gui_pos: Some((12, 34)),
        gui_size: Some((640, 480)),
    }];
    assert!(app.settings.save_checked());
    let paths = json!([added, existing]);
    let result = app
        .agent_set_settings(&changes(json!({"vst3_plugins":paths})), "vst-paths", &ctx)
        .unwrap();
    assert_eq!(detail(&app, "vst3_plugins").value, paths);
    let saved = Settings::load();
    for entries in [&app.settings.vst3_plugins, &saved.vst3_plugins] {
        assert_eq!(
            entries[1].state.as_deref(),
            Some("PRIVATE_RUNNING_VST_STATE")
        );
        assert!(entries[1].bypass && entries[1].user_hidden);
        assert_eq!(entries[1].gui_pos, Some((12, 34)));
        assert_eq!(entries[1].gui_size, Some((640, 480)));
        assert!(entries[0].state.is_none());
    }
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("PRIVATE_RUNNING_VST_STATE")
    );
    assert!(app.pref_state.is_none());
    assert_final_budget(&result);
}

#[test]
fn agent_settings_listing_covers_the_catalog_once_and_keeps_details_explicit() {
    let ctx = egui::Context::default();
    let app = fixture(&ctx);
    let expected = crate::settings_transfer::capture_stage1(
        &app.settings.preferences_snapshot(),
        false,
        false,
    );
    let first = app.agent_get_settings(None, None, None).unwrap();
    let pages = first.pages;
    assert_eq!(pages as usize, expected.len().div_ceil(SETTINGS_PAGE_SIZE));
    assert!(pages > 1);
    let mut index = 0;
    let mut keys = HashSet::new();
    for page in 1..=pages {
        let result = app.agent_get_settings(None, None, Some(page)).unwrap();
        assert_eq!(result.page, page);
        assert_eq!(result.pages, pages);
        assert!(!result.more);
        assert!(result.items.len() <= SETTINGS_PAGE_SIZE);
        assert_final_budget(&result);
        for row in result.items {
            let expected = &expected[index];
            match row {
                PageRow::Value(row) => {
                    assert_eq!(row.key, expected.key);
                    assert_eq!(row.label, expected.label);
                    assert_eq!(row.value, expected.value);
                    assert!(row.choices.is_none());
                    assert!(row.template.is_none());
                }
                PageRow::TooLong(marker) => {
                    assert_eq!(
                        marker.id,
                        mimageviewer_agent_ipc::wire_digest(&expected.key).unwrap()
                    );
                }
            }
            assert!(keys.insert(expected.key.clone()));
            index += 1;
        }
    }
    assert_eq!(index, expected.len());
    for excluded in [
        "agent_level",
        "agent_allow_settings",
        "remote_service_enabled",
        "keymap",
    ] {
        assert!(!keys.contains(excluded));
    }
    let wheel = detail(&app, "fullscreen_wheel_action");
    let choices = wheel.choices.unwrap().unwrap();
    assert!(
        choices
            .iter()
            .any(|choice| choice.value == json!("PageTurn"))
    );
    assert!(choices.iter().all(|choice| !choice.label.is_empty()));
    assert!(
        detail(&app, "slideshow_interval_secs")
            .choices
            .unwrap()
            .is_none()
    );
    assert!(detail(&app, "external_tools").template.is_some());
    assert_no_publication(&app);
}

#[test]
fn agent_settings_detail_and_page_budgets_keep_stable_omitted_row_identities() {
    let ctx = egui::Context::default();
    let mut app = fixture(&ctx);
    let mut tool = crate::external_tool::ExternalTool::defaults_for_viewing();
    tool.arguments = "\\\"\n".repeat(MAX_ITEM_CHARS);
    app.settings.external_tools = vec![tool];
    let keys = [
        "external_tools".to_owned(),
        "fullscreen_wheel_action".to_owned(),
    ];
    let page = app.agent_get_settings(None, Some(&keys), None).unwrap();
    assert_eq!(page.items.len(), 2);
    let marker = page
        .items
        .iter()
        .find_map(|row| match row {
            PageRow::TooLong(marker) => Some(marker),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        marker.id,
        mimageviewer_agent_ipc::wire_digest("external_tools").unwrap()
    );
    assert!(
        page.items.iter().any(
            |row| matches!(row, PageRow::Value(value) if value.key == "fullscreen_wheel_action")
        )
    );
    assert_final_budget(&page);

    let large_path = PathBuf::from(format!("C:/{}", "x".repeat(5_500)));
    app.settings.startup_folder_path = Some(large_path.clone());
    app.settings.book_root = Some(large_path.clone());
    app.settings.capture_output_dir = Some(large_path.clone());
    app.settings.clipboard_capture_output_dir = Some(large_path);
    let keys: Vec<String> = [
        "startup_folder_path",
        "book_root",
        "capture_output_dir",
        "clipboard_capture_output_dir",
    ]
    .map(str::to_owned)
    .into();
    for key in &keys {
        let row = detail(&app, key);
        assert!(mcp_content_chars(&row).unwrap() < MAX_ITEM_CHARS);
    }
    let page = app.agent_get_settings(None, Some(&keys), None).unwrap();
    assert_eq!(page.items.len(), keys.len());
    assert!(
        page.items
            .iter()
            .any(|row| matches!(row, PageRow::TooLong(_)))
    );
    assert!(
        page.items
            .iter()
            .any(|row| matches!(row, PageRow::Value(_)))
    );
    assert_final_budget(&page);
    assert_no_publication(&app);
}

#[test]
fn agent_settings_get_validates_queries_key_counts_and_page_ranges() {
    let ctx = egui::Context::default();
    let app = fixture(&ctx);
    let found = app
        .agent_get_settings(Some("FULLSCREEN WHEEL"), None, None)
        .unwrap();
    assert!(!found.items.is_empty());
    assert!(
        found
            .items
            .iter()
            .any(|row| matches!(row, PageRow::Value(row) if row.key == "fullscreen_wheel_action"))
    );
    assert!(
        found
            .items
            .iter()
            .all(|row| matches!(row, PageRow::Value(row) if row.choices.is_some()))
    );
    let empty = app
        .agent_get_settings(Some("definitely-no-such-setting-8731"), None, None)
        .unwrap();
    assert!(empty.items.is_empty());
    assert!(!empty.more);
    let capped = app.agent_get_settings(Some(""), None, None).unwrap();
    assert_eq!(capped.items.len(), MAX_SETTINGS_KEYS);
    assert!(capped.more);
    assert_final_budget(&capped);
    for result in [
        app.agent_get_settings(Some(&"x".repeat(201)), None, None),
        app.agent_get_settings(None, Some(&[]), None),
        app.agent_get_settings(
            None,
            Some(&vec![
                "fullscreen_wheel_action".into();
                MAX_SETTINGS_KEYS + 1
            ]),
            None,
        ),
        app.agent_get_settings(None, None, Some(0)),
        app.agent_get_settings(None, None, Some(u32::MAX)),
        app.agent_get_settings(Some("wheel"), None, Some(2)),
    ] {
        assert_eq!(result.unwrap_err().code, ErrorCode::BadRequest);
    }
    assert_no_publication(&app);
}

#[test]
fn agent_settings_queued_get_and_set_recheck_live_admission_without_activation() {
    let ctx = egui::Context::default();
    for write in [false, true] {
        for mode in ["allowed", "permission", "level", "startup", "remote"] {
            let mut app = fixture(&ctx);
            app.settings.fullscreen_wheel_action = FullscreenWheelAction::PageTurn;
            assert!(app.settings.save_checked());
            let session = crate::remote_ipc::session::SessionHandle::new();
            app.set_remote_session_handle(session.clone());
            let controller = OpenController::new(Some(session.clone()));
            app.install_agent_open(controller.clone(), &ctx);
            let (ready, queued) = mpsc::channel();
            controller.set_enqueue_observer(ready);
            let kind = if write {
                UiRequestKind::SetSettings {
                    changes: changes(json!({"fullscreen_wheel_action": "InPageScroll"})),
                    client_name: "queued-settings".into(),
                }
            } else {
                UiRequestKind::GetSettings {
                    query: None,
                    keys: Some(vec!["fullscreen_wheel_action".into()]),
                    page: None,
                }
            };
            let worker = std::thread::spawn(move || controller.execute_settings_for_test(kind));
            queued.recv_timeout(Duration::from_secs(10)).unwrap();
            match mode {
                "permission" => app.settings.agent_allow_settings = false,
                "level" => app.settings.agent_level = AgentLevel::Disabled,
                "startup" => app.initialized = false,
                "remote" => acquire_remote(&session),
                "allowed" => {}
                _ => unreachable!(),
            }
            app.poll_agent_open_requests(&ctx);
            let result = worker.join().unwrap().unwrap();
            match (mode, write, result) {
                (
                    "allowed",
                    true,
                    UiOpenOutcome::Written(Ok(mimageviewer_agent_ipc::Payload::SettingsWrite(
                        result,
                    ))),
                ) => {
                    assert_eq!(result.changed.len(), 1);
                    assert_eq!(
                        app.settings.fullscreen_wheel_action,
                        FullscreenWheelAction::InPageScroll
                    );
                    assert_eq!(
                        Settings::load().fullscreen_wheel_action,
                        FullscreenWheelAction::InPageScroll
                    );
                }
                ("allowed", false, UiOpenOutcome::Settings(page)) => {
                    assert_eq!(page.items.len(), 1);
                    assert_no_publication(&app);
                }
                (mode, _, UiOpenOutcome::Refused(error)) if mode != "allowed" => {
                    assert_eq!(
                        error.code,
                        match mode {
                            "startup" => ErrorCode::NotReady,
                            "remote" => ErrorCode::Busy,
                            _ => ErrorCode::Disabled,
                        }
                    );
                    assert_eq!(
                        app.settings.fullscreen_wheel_action,
                        FullscreenWheelAction::PageTurn
                    );
                    assert_eq!(
                        Settings::load().fullscreen_wheel_action,
                        FullscreenWheelAction::PageTurn
                    );
                    assert_no_publication(&app);
                }
                (mode, write, result) => {
                    panic!("unexpected queued settings outcome: {mode}, write={write}: {result:?}")
                }
            }
            assert!(app.pref_state.is_none());
            assert!(!app.show_preferences);
            assert!(app.startup_open_path_resolve_pending.is_none());
            assert!(app.activation_open_path_rx.try_recv().is_err());
        }
    }
}
