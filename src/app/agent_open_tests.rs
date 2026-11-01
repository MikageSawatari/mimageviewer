//! Headless coverage of the agent/UI admission boundary. No native window is opened.
#![cfg(windows)]

use super::*;
use crate::agent_ipc::open::OpenController;
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use mimageviewer_agent_ipc::{AgentError, AgentLevel, ErrorCode, OpenResult, OpenStatus};

struct QueuedOpen {
    cancel: Arc<AtomicBool>,
    worker: std::thread::JoinHandle<Result<OpenResult, AgentError>>,
}

impl QueuedOpen {
    fn finish(self) -> Result<OpenResult, AgentError> {
        self.worker.join().expect("agent open worker")
    }
}

fn install(app: &mut App, ctx: &egui::Context) -> Arc<OpenController> {
    app.initialized = true;
    app.settings.agent_level = AgentLevel::ReadAndOpen;
    let controller = OpenController::new(app.remote_session_handle().cloned());
    app.install_agent_open(Arc::clone(&controller), ctx);
    controller
}

fn enqueue(controller: &Arc<OpenController>, path: &Path) -> QueuedOpen {
    let (ready, queued) = mpsc::channel();
    controller.set_enqueue_observer(ready);
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let worker_controller = Arc::clone(controller);
    let path = path.to_string_lossy().into_owned();
    let worker = std::thread::spawn(move || worker_controller.execute(path, &worker_cancel));
    queued
        .recv_timeout(Duration::from_secs(10))
        .expect("worker queued the UI request");
    QueuedOpen { cancel, worker }
}

fn arm_document_modal(app: &mut AppTestEnv, ctx: &egui::Context) {
    use crate::clipboard_capture::{
        CaptureIntent, SelectionSnapshot, data::CaptureTimestamp, html::HtmlCapture,
    };
    let snapshot = Arc::new(SelectionSnapshot {
        token: 1,
        html_epoch: 0,
        html: HtmlCapture {
            page_url: "https://example.invalid/agent-open-test".into(),
            candidates: vec!["data:image/png;base64,AA==".into()],
            omitted: 0,
        },
        intent: CaptureIntent::Manual {
            destination: app.tmp.path().join("paste"),
        },
        timestamp: CaptureTimestamp {
            stamp: "20261031-120000-000".into(),
            month: "2026-10".into(),
        },
    });
    app.handle_capture_selection(snapshot, ctx, |_, _| {
        panic!("manual capture must not activate a window")
    });
    assert!(app.document_open_modal_admission_blocked());
}

fn assert_refused_without_open(app: &App, queued: QueuedOpen, reason: &str) {
    let error = queued.finish().expect_err("UI must refuse the request");
    assert_eq!(error.code, ErrorCode::Busy);
    assert!(error.message.contains(reason), "{}", error.message);
    assert!(app.startup_open_path_resolve_pending.is_none());
    assert!(app.activation_open_path_rx.try_recv().is_err());
}

#[test]
fn agent_open_ui_refuses_composition_editor_document_modal_and_sidecar() {
    for refusal in [
        ActivationOpenRefusal::CompositionEditor,
        ActivationOpenRefusal::DocumentModal,
        ActivationOpenRefusal::SidecarRestore,
    ] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let controller = install(&mut app, &ctx);
        let path = app.tmp.path().to_path_buf();
        let queued = enqueue(&controller, &path);
        let reason = match refusal {
            ActivationOpenRefusal::CompositionEditor => {
                app.composition_editor_operations.owner = Some(
                    crate::app::book_composition_editor::CompositionEditorOwner {
                        context_id: app.projected_viewer_context_id(),
                        scope: crate::book_scope::BookScopeKey::document(&path.join("book.cbz"))
                            .unwrap(),
                        request_id: 1,
                    },
                );
                "構成"
            }
            ActivationOpenRefusal::DocumentModal => {
                arm_document_modal(&mut app, &ctx);
                "処理"
            }
            ActivationOpenRefusal::SidecarRestore => {
                app.activate_sidecar_restore_modal_for_test(path);
                "復元"
            }
            _ => unreachable!(),
        };
        assert!(matches!(app.activation_open_precheck(None), Some(actual) if actual == refusal));
        app.poll_agent_open_requests(&ctx);
        assert_refused_without_open(&app, queued, reason);
    }
}

#[test]
fn agent_open_ui_rechecks_remote_ownership_after_worker_enqueue() {
    let mut app = setup_app();
    let session = crate::remote_ipc::session::SessionHandle::new();
    app.set_remote_session_handle(session.clone());
    let ctx = egui::Context::default();
    let controller = install(&mut app, &ctx);
    let queued = enqueue(&controller, app.tmp.path());
    session.acquire(mimageviewer_ipc::SessionAcquireRequest {
        client_id: "agent-ui-admission-test".into(),
        peer: mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: None,
        },
    });
    app.poll_agent_open_requests(&ctx);
    assert_refused_without_open(&app, queued, "リモート");
}

#[test]
fn agent_open_startup_guard_refuses_overlay_and_uninitialized_without_deferring() {
    for overlay in [false, true] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let controller = install(&mut app, &ctx);
        let queued = enqueue(&controller, app.tmp.path());
        app.initialized = overlay;
        app.indexer_init = if overlay {
            IndexerInit::NotStarted
        } else {
            IndexerInit::Unavailable
        };
        app.reject_agent_open_requests_during_startup();
        let error = queued
            .finish()
            .expect_err("startup request must be refused");
        assert_eq!(error.code, ErrorCode::NotReady);
        assert!(app.startup_open_path_resolve_pending.is_none());
        app.initialized = true;
        app.indexer_init = IndexerInit::Unavailable;
        app.poll_agent_open_requests(&ctx);
        assert!(app.startup_open_path_resolve_pending.is_none());
    }
}

#[test]
fn agent_open_ui_rechecks_permission_drop_before_admission() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let controller = install(&mut app, &ctx);
    let queued = enqueue(&controller, app.tmp.path());
    app.settings.agent_level = AgentLevel::ReadOnly;
    app.poll_agent_open_requests(&ctx);
    assert_eq!(queued.finish().unwrap_err().code, ErrorCode::Disabled);
    assert!(app.startup_open_path_resolve_pending.is_none());
}

#[test]
fn agent_open_cancelled_queued_request_never_starts_resolver() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let controller = install(&mut app, &ctx);
    let queued = enqueue(&controller, app.tmp.path());
    queued.cancel.store(true, Ordering::Release);
    assert_eq!(queued.finish().unwrap_err().code, ErrorCode::Cancelled);
    app.poll_agent_open_requests(&ctx);
    assert!(app.startup_open_path_resolve_pending.is_none());
    assert!(app.activation_open_path_rx.try_recv().is_err());
}

#[test]
fn agent_open_is_accepted_then_superseded_by_same_frame_second_instance() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let controller = install(&mut app, &ctx);
    let agent_path = app.tmp.path().join("agent");
    let user_path = app.tmp.path().join("second-instance");
    std::fs::create_dir(&agent_path).unwrap();
    std::fs::create_dir(&user_path).unwrap();
    let queued = enqueue(&controller, &agent_path);
    app.activation_open_path_tx
        .send(crate::single_instance::ActivationOpenPath::received(
            user_path.clone(),
            None,
        ))
        .unwrap();
    app.poll_agent_open_requests(&ctx);
    let pending = app.startup_open_path_resolve_pending.as_ref().unwrap();
    assert_eq!(pending.requested.as_path(), agent_path.as_path());
    assert!(matches!(pending.owner, StartupOpenPathOwner::Activation));
    let cancelled_by_user = Arc::clone(&pending.cancel);
    let accepted = queued.finish().expect("agent request was admitted");
    assert_eq!(accepted.status, OpenStatus::Accepted);
    app.poll_activation_open_paths(&ctx);
    assert!(cancelled_by_user.load(Ordering::Relaxed));
    assert_eq!(
        app.startup_open_path_resolve_pending
            .as_ref()
            .unwrap()
            .requested
            .as_path(),
        user_path.as_path(),
    );
}

#[test]
fn agent_open_resolver_refuses_document_modal_while_second_instance_requeues() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let path = app.tmp.path().to_path_buf();
    arm_document_modal(&mut app, &ctx);
    assert!(matches!(
        app.start_startup_open_path_resolve(path.clone(), StartupOpenPathSource::Agent, &ctx),
        StartupOpenAdmission::Refused(ActivationOpenRefusal::DocumentModal),
    ));
    assert!(app.activation_open_path_rx.try_recv().is_err());
    assert!(matches!(
        app.start_startup_open_path_resolve(path.clone(), StartupOpenPathSource::Activation, &ctx),
        StartupOpenAdmission::Requeued,
    ));
    assert_eq!(app.activation_open_path_rx.try_recv().unwrap().path, path);
    assert!(app.startup_open_path_resolve_pending.is_none());
}

#[test]
fn agent_open_resolver_refuses_epub_deletion_while_second_instance_requeues() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let path = app.tmp.path().join("deleting.epub");
    std::fs::write(&path, b"fixture").unwrap();
    let _deletion = crate::pdf_loader::acquire_epub_delete_coverage(&path).unwrap();
    assert!(matches!(
        app.start_startup_open_path_resolve(path.clone(), StartupOpenPathSource::Agent, &ctx),
        StartupOpenAdmission::Refused(ActivationOpenRefusal::EpubDeleting),
    ));
    assert!(app.activation_open_path_rx.try_recv().is_err());
    assert!(matches!(
        app.start_startup_open_path_resolve(path.clone(), StartupOpenPathSource::Activation, &ctx),
        StartupOpenAdmission::Requeued,
    ));
    assert_eq!(app.activation_open_path_rx.try_recv().unwrap().path, path);
    assert!(app.startup_open_path_resolve_pending.is_none());
}
