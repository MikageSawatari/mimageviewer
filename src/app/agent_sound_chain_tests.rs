//! Exercise the real UI admission turn without loading a host or a plug-in.
#![cfg(windows)]
use super::*;
use crate::agent_ipc::changes::ChangeController;
use crate::agent_ipc::open::{OpenController, UiOpenOutcome, UiRequestKind};
use crate::app::tests::phase_c_support::setup_app;
use crate::effetune::sound_document::GetInput;
use crate::effetune::sound_service::{SoundError, SoundOutcome, SoundRequest, SoundResponse};
use crate::effetune::{EffetuneFailure, EffetuneRuntime, LoadOrigin};
use mimageviewer_agent_ipc::{AgentLevel, ErrorCode};
use std::sync::{Arc, mpsc};
use std::time::Duration;

#[derive(Debug)]
enum WorkerResult {
    Refused(ErrorCode),
    Admitted(Result<SoundResponse, SoundError>),
}

fn enqueue_on_worker(
    controller: Arc<OpenController>,
    request: SoundRequest,
) -> (mpsc::Receiver<()>, std::thread::JoinHandle<WorkerResult>) {
    let (ready, queued) = mpsc::channel();
    controller.set_enqueue_observer(ready);
    let worker = std::thread::spawn(move || {
        match controller
            .execute_settings_for_test(UiRequestKind::SoundChain {
                request,
                client_name: "sound-ui-fixture".into(),
            })
            .unwrap()
        {
            UiOpenOutcome::Refused(error) => WorkerResult::Refused(error.code),
            UiOpenOutcome::SoundChainAdmitted(ticket) => {
                // Only this worker consumes the ticket. An unfinished service
                // returns Pending here without blocking the UI or starting a host.
                WorkerResult::Admitted(ticket.with_wait_budget(Duration::ZERO).wait())
            }
            outcome => panic!("unexpected sound admission outcome: {outcome:?}"),
        }
    });
    (queued, worker)
}

#[test]
fn sound_chain_ui_admission_rechecks_live_permission_startup_dialogs_and_remote() {
    let ctx = egui::Context::default();
    for read in [false, true] {
        for mode in [
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
            "changes-not-installed",
        ] {
            // AppTestEnv keeps the disposable data-dir override alive until
            // after App and its workers have been dropped.
            let mut app = setup_app();
            app.initialized = true;
            app.indexer_init = IndexerInit::Unavailable;
            app.settings.agent_level = AgentLevel::ReadOnly;
            app.settings.agent_allow_settings = true;
            let runtime = EffetuneRuntime::Failed(EffetuneFailure::LoadFailed("fixture".into()));
            app.effetune.runtime = runtime.clone();
            let changes = ChangeController::new();
            app.install_agent_changes(Arc::clone(&changes), &ctx);
            let session = crate::remote_ipc::session::SessionHandle::new();
            app.set_remote_session_handle(session.clone());
            let controller = OpenController::new(Some(session.clone()));
            app.install_agent_open(controller.clone(), &ctx);
            let request = if read {
                SoundRequest::Get(GetInput::default())
            } else {
                SoundRequest::Lock
            };
            let (queued, worker) = enqueue_on_worker(controller, request);
            queued.recv_timeout(Duration::from_secs(10)).unwrap();
            // Permission at enqueue time is insufficient: the UI must inspect
            // these live values in the same turn that would submit the job.
            let expected = match mode {
                "permission" => {
                    app.settings.agent_allow_settings = false;
                    ErrorCode::Disabled
                }
                "level" => {
                    app.settings.agent_level = AgentLevel::Disabled;
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
                    let snapshot = app.settings.preferences_snapshot();
                    app.install_headless_preferences_state(snapshot);
                    ErrorCode::Busy
                }
                "fav-add" => {
                    app.show_fav_add_dialog = true;
                    ErrorCode::Busy
                }
                "fav-edit" => {
                    app.show_favorites_editor = true;
                    ErrorCode::Busy
                }
                "remote" => {
                    session.acquire(mimageviewer_ipc::SessionAcquireRequest {
                        client_id: "sound-ui-fixture".into(),
                        peer: mimageviewer_ipc::SessionPeerInfo {
                            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                            device_name: None,
                        },
                    });
                    ErrorCode::Busy
                }
                "changes-not-installed" => {
                    app.agent_changes.take();
                    ErrorCode::NotReady
                }
                _ => unreachable!(),
            };
            let before = serde_json::to_value(&app.settings).unwrap();
            let draft = app
                .pref_state
                .as_ref()
                .map(|state| std::ptr::from_ref(state.as_ref()));
            let generation = crate::settings::save_generation();
            app.poll_agent_open_requests(&ctx);
            match worker.join().unwrap() {
                WorkerResult::Refused(code) => assert_eq!(code, expected, "{mode}, read={read}"),
                outcome => panic!("unexpected {mode}, read={read}: {outcome:?}"),
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
                    .map(|state| std::ptr::from_ref(state.as_ref())),
                draft
            );
            assert_eq!(app.effetune.runtime, runtime, "{mode}");
            assert!(app.effetune.bridge().is_none(), "{mode}");
            // Failed would immediately publish a service failure if admission
            // accidentally reached submit_sound. Refusal must not reach it.
            assert!(
                changes.records().lock().unwrap().entries().is_empty(),
                "{mode}"
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
}

#[test]
fn sound_chain_ui_admits_ticket_and_worker_waits_without_loading_or_saving() {
    let ctx = egui::Context::default();
    for loading in [false, true] {
        let mut app = setup_app();
        app.initialized = true;
        app.indexer_init = IndexerInit::Unavailable;
        app.settings.agent_level = AgentLevel::ReadOnly;
        app.settings.agent_allow_settings = true;
        let runtime = if loading {
            // No actual load worker exists. This state deliberately leaves
            // the service pending so a UI-side wait would never complete.
            EffetuneRuntime::Loading {
                origin: LoadOrigin::SoundAgent,
                open_gui_when_ready: None,
            }
        } else {
            EffetuneRuntime::Failed(EffetuneFailure::LoadFailed("fixture".into()))
        };
        app.effetune.runtime = runtime.clone();
        app.install_agent_changes(ChangeController::new(), &ctx);
        let controller = OpenController::new(None);
        app.install_agent_open(controller.clone(), &ctx);
        let (queued, worker) = enqueue_on_worker(controller, SoundRequest::Lock);
        queued.recv_timeout(Duration::from_secs(10)).unwrap();
        let before = serde_json::to_value(&app.settings).unwrap();
        let generation = crate::settings::save_generation();
        let state_path = app.tmp.path().join("effetune/mixwright-state.json");
        let persisted = std::fs::read(&state_path).ok();
        app.poll_agent_open_requests(&ctx);
        match (loading, worker.join().unwrap()) {
            (true, WorkerResult::Admitted(Ok(response))) => {
                assert!(matches!(response.outcome, SoundOutcome::Pending { .. }));
                assert!(response.persistence_warning.is_none());
            }
            (false, WorkerResult::Admitted(Err(SoundError::Unavailable(reason)))) => {
                assert_eq!(reason, "読み込みに失敗しました");
            }
            (_, outcome) => panic!("unexpected loading={loading}: {outcome:?}"),
        }
        assert_eq!(serde_json::to_value(&app.settings).unwrap(), before);
        assert_eq!(crate::settings::save_generation(), generation);
        assert_eq!(std::fs::read(&state_path).ok(), persisted);
        assert_eq!(app.effetune.runtime, runtime);
        assert!(app.effetune.bridge().is_none());
        assert!(app.pref_state.is_none());
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
