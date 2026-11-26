//! Admission and file submission are one UI turn; rejected work never enters the FIFO.
#![cfg(windows)]
use super::*;
use crate::agent_ipc::open::{OpenController, UiOpenOutcome};
use crate::app::tests::phase_c_support::setup_app;
use mimageviewer_agent_ipc::{AgentLevel, ErrorCode};
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn operation_script_ui_admission_rechecks_live_permission_modal_preferences_and_remote() {
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
        let temp = tempfile::tempdir().unwrap();
        let touched = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = touched.clone();
        let mut app = TestApp(setup_app());
        app.operation_script = crate::operation_script::OperationScriptLoader::with_file_worker(
            crate::operation_script::script_path(temp.path()),
            |_| Ok(()),
            move || {
                observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            },
        );
        app.initialized = true;
        app.indexer_init = IndexerInit::Unavailable;
        app.settings.agent_level = AgentLevel::ReadOnly;
        app.settings.agent_allow_settings = true;
        app.settings.operation_script_enabled = true;
        let session = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(session.clone());
        let controller = OpenController::new(Some(session.clone()));
        app.install_agent_open(controller.clone(), &ctx);
        let (ready, queued) = mpsc::channel();
        controller.set_enqueue_observer(ready);
        let (reply, received) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            controller.execute_settings_for_test(
                crate::agent_ipc::open::UiRequestKind::OperationScript(
                    crate::operation_script::AgentScriptRequest {
                        source: None,
                        expected_sha256: None,
                        reply,
                    },
                ),
            )
        });
        queued.recv_timeout(Duration::from_secs(10)).unwrap();
        // Change live admission only after the worker has queued its request.
        // A rejected request must not submit file work.
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
                app.settings.operation_script_enabled = false;
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
            (None, UiOpenOutcome::OperationScriptAdmitted { snapshot, queued }) => {
                queued.unwrap();
                assert_eq!(snapshot.enabled_at_admission, mode == "enabled");
                assert_eq!(
                    snapshot.load_state_at_admission,
                    mimageviewer_agent_ipc::OperationScriptLoadState::Disabled
                );
                let result = received
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap()
                    .unwrap();
                assert!(
                    matches!(result, crate::operation_script::AgentScriptOutcome::Read(document) if document.builtin)
                );
            }
            (Some(code), UiOpenOutcome::Refused(error)) => assert_eq!(error.code, code, "{mode}"),
            (_, outcome) => panic!("unexpected {mode}: {outcome:?}"),
        }
        app.operation_script.shutdown_file_worker_for_test();
        assert_eq!(
            touched.load(std::sync::atomic::Ordering::Relaxed),
            usize::from(expected.is_none()),
            "{mode}"
        );
        assert!(!crate::operation_script::script_path(temp.path()).exists());
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

// Joining in Drop also protects disposable data if an assertion unwinds.
struct TestApp(crate::app::tests::phase_c_support::AppTestEnv);
impl std::ops::Deref for TestApp {
    type Target = App;
    fn deref(&self) -> &App {
        &self.0.app
    }
}
impl std::ops::DerefMut for TestApp {
    fn deref_mut(&mut self) -> &mut App {
        &mut self.0.app
    }
}
impl Drop for TestApp {
    fn drop(&mut self) {
        self.0.app.operation_script.shutdown_file_worker_for_test();
    }
}
