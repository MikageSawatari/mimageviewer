//! The UI only admits; file work and FIFO waiting belong to the agent worker.
#![cfg(windows)]
use super::*;
use crate::agent_ipc::open::{OpenController, UiOpenOutcome};
use crate::app::tests::phase_c_support::setup_app;
use mimageviewer_agent_ipc::{AgentLevel, ErrorCode};
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn stack_script_ui_admission_rechecks_live_permission_modal_preferences_and_remote() {
    let ctx = egui::Context::default();
    for mode in [
        "enabled",
        "disabled-option",
        "permission",
        "level",
        "startup",
        "overlay",
        "modal",
        "preferences",
        "draft",
        "fav-add",
        "fav-edit",
        "remote",
    ] {
        let mut app = setup_app();
        app.initialized = true;
        app.indexer_init = IndexerInit::Unavailable;
        app.settings.agent_level = AgentLevel::ReadOnly;
        app.settings.agent_allow_settings = true;
        app.settings.stack_script_enabled = true;
        let session = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(session.clone());
        let controller = OpenController::new(Some(session.clone()));
        app.install_agent_open(controller.clone(), &ctx);
        let (ready, queued) = mpsc::channel();
        controller.set_enqueue_observer(ready);
        let worker = std::thread::spawn(move || controller.stack_script_admission_for_test());
        queued.recv_timeout(Duration::from_secs(10)).unwrap();
        // Change live admission only after the worker has queued its request.
        // No source text or file operation is part of this UI request.
        let expected = match mode {
            "permission" => {
                app.settings.agent_allow_settings = false;
                Some(ErrorCode::Disabled)
            }
            "level" => {
                app.settings.agent_level = AgentLevel::Disabled;
                Some(ErrorCode::Disabled)
            }
            "startup" => {
                app.initialized = false;
                Some(ErrorCode::NotReady)
            }
            "overlay" => {
                app.indexer_init = IndexerInit::NotStarted;
                Some(ErrorCode::NotReady)
            }
            "modal" => {
                app.show_about_dialog = true;
                Some(ErrorCode::Busy)
            }
            "preferences" => {
                app.show_preferences = true;
                Some(ErrorCode::Busy)
            }
            "draft" => {
                let snapshot = app.settings.preferences_snapshot();
                app.install_headless_preferences_state(snapshot);
                Some(ErrorCode::Busy)
            }
            "fav-add" => {
                app.show_fav_add_dialog = true;
                Some(ErrorCode::Busy)
            }
            "fav-edit" => {
                app.show_favorites_editor = true;
                Some(ErrorCode::Busy)
            }
            "remote" => {
                session.acquire(mimageviewer_ipc::SessionAcquireRequest {
                    client_id: "stack-script-test".into(),
                    peer: mimageviewer_ipc::SessionPeerInfo {
                        connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                        device_name: None,
                    },
                });
                Some(ErrorCode::Busy)
            }
            "disabled-option" => {
                app.settings.stack_script_enabled = false;
                None
            }
            "enabled" => None,
            _ => unreachable!(),
        };
        let before = serde_json::to_value(&app.settings).unwrap();
        let draft = app
            .pref_state
            .as_ref()
            .map(|draft| std::ptr::from_ref(draft.as_ref()));
        let generation = crate::settings::save_generation();
        app.poll_agent_open_requests(&ctx);
        match (expected, worker.join().unwrap().unwrap()) {
            (None, UiOpenOutcome::StackScriptAdmitted { enabled }) => {
                assert_eq!(enabled, mode == "enabled")
            }
            (Some(code), UiOpenOutcome::Refused(error)) => assert_eq!(error.code, code, "{mode}"),
            (_, outcome) => panic!("unexpected {mode}: {outcome:?}"),
        }
        assert_eq!(
            serde_json::to_value(&app.settings).unwrap(),
            before,
            "{mode}"
        );
        assert_eq!(crate::settings::save_generation(), generation, "{mode}");
        assert_eq!(
            app.pref_state
                .as_ref()
                .map(|draft| std::ptr::from_ref(draft.as_ref())),
            draft
        );
        assert!(app.startup_open_path_resolve_pending.is_none());
        assert!(app.activation_open_path_rx.try_recv().is_err());
        assert!(
            app.agent_open_requests
                .as_ref()
                .unwrap()
                .try_recv()
                .is_err()
        );
    }
}
