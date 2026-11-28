//! Controller/queue integration against the in-memory host transport only.
//! Never call start_load: its production worker can launch the native host.

use super::sound_service::{SoundError, SoundOutcome, SoundRequest};
use super::*;
use crate::agent_ipc::changes::{ChangeController, ChangeRowOutcome, ChangeRowResult};
use crate::video::dsp::Event;
use crate::video::dsp::bridge::{Bridge, SoundLockRelease, SoundReply};
use serde_json::json;

const WAIT: Duration = Duration::from_secs(5);

fn loading_controller(path: &Path) -> (EffetuneController, mpsc::Receiver<HostCommand>) {
    let mut controller = EffetuneController::new();
    // Replace the dormant default queue before any request can reach it. Every
    // capture and sound-service persistence below belongs to this temp path.
    controller.captures = Arc::new(CaptureQueue::new(path.to_path_buf()));
    let (host_tx, host_rx) = mpsc::channel();
    controller.host_tx = host_tx;
    controller.runtime = EffetuneRuntime::Loading {
        origin: LoadOrigin::SoundAgent,
        open_gui_when_ready: None,
    };
    (controller, host_rx)
}

fn fake_dsp(directory: &Path) -> Arc<DspBridge> {
    // This constructor only creates the owner. The fake host is installed by
    // each test; enable()/load_worker()/native GUI entry points are never used.
    DspBridge::new_for_miv_sound(directory.join("effetune"))
}

fn state(tag: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "formatVersion": 1, "appVersion": "test", "pipelineA": [
            {"id": 1, "type": "VolumePlugin", "name": "Volume", "enabled": true,
                "parameters": {"vl": -3}}
        ], "pipelineB": null, "currentPipeline": "A", "masterBypass": false,
        "oversampling": {"factor": 1}, "ui": {"test_tag": tag}
    }))
    .unwrap()
}

fn command(commands: &mpsc::Receiver<Value>, expected: &str) -> Value {
    let command = commands
        .recv_timeout(WAIT)
        .expect("serial worker must issue the next command");
    assert_eq!(command["cmd"], expected);
    assert_eq!(command["slot_id"], 0);
    assert!(command["request_id"].as_u64().is_some());
    command
}

fn reply(command: &Value, status: &str, state: Option<&[u8]>) -> SoundReply {
    SoundReply {
        request_id: command["request_id"].as_u64().unwrap(),
        slot_id: 0,
        status: status.into(),
        lock: Some("controller-test-lock".into()),
        generation: 0,
        state: state.map(|data| base64::engine::general_purpose::STANDARD.encode(data)),
        guard: None,
    }
}

#[test]
fn sound_controller_loading_holds_request_without_opening_then_resumes_after_fake_completion() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("mixwright-state.json");
    let (mut controller, host_commands) = loading_controller(&state_path);
    let changes = ChangeController::new();
    let (host, commands, events) = Bridge::fake_host_for_test();
    let bridge = fake_dsp(directory.path());
    bridge.install_fake_host_for_test(Arc::clone(&host));

    let ticket = controller.submit_sound(
        SoundRequest::Lock,
        "test client",
        Arc::clone(&changes),
        0.0,
        None,
    );
    assert!(matches!(
        controller.runtime,
        EffetuneRuntime::Loading {
            origin: LoadOrigin::SoundAgent,
            open_gui_when_ready: None
        }
    ));
    assert_eq!(controller.sound_pending_load.len(), 1);
    assert!(controller.has_pending_ui_work());
    assert!(controller.bridge().is_none());
    assert!(controller.slot.snapshot().is_none());
    assert!(
        commands.try_recv().is_err(),
        "unpublished bridge must receive no request"
    );
    assert!(
        host_commands.try_recv().is_err(),
        "sound admission must not request an editor"
    );
    assert!(!state_path.exists());

    controller.set_test_load_bridge_completion(Arc::clone(&bridge));
    assert!(controller.poll());
    assert!(matches!(
        controller.runtime,
        EffetuneRuntime::Running { .. }
    ));
    assert!(controller.sound_pending_load.is_empty());
    assert!(controller.slot.snapshot().is_some());
    assert!(Arc::ptr_eq(controller.bridge().unwrap(), &bridge));
    assert!(matches!(
        host_commands.recv_timeout(WAIT).unwrap(),
        HostCommand::RegisterBridge(_)
    ));
    assert!(
        host_commands.try_recv().is_err(),
        "load completion must not show or attach a GUI"
    );

    // submit_sound and poll returned while no host reply was available. Pipe
    // commands and the state wait therefore belong to the queue worker.
    let lock = command(&commands, "sound_lock");
    events.emit(Event::SoundLock(reply(&lock, "Locked", None)));
    let persist = command(&commands, "sound_get");
    let saved = state("loaded");
    events.emit(Event::SoundGet(reply(&persist, "State", Some(&saved))));
    let response = ticket.wait().unwrap();
    assert_eq!(
        response.outcome,
        SoundOutcome::Locked {
            lock: "controller-test-lock".into(),
            revision: "controller-test-lock:0".into()
        }
    );
    assert!(response.persistence_warning.is_none());
    assert_eq!(fs::read(&state_path).unwrap(), saved);
    assert_eq!(
        host.fake_command_threads_for_test(),
        vec!["effetune-state-capture"; 2]
    );
    assert!(!bridge.slot(0).unwrap().gui_visible);
    assert!(commands.try_recv().is_err());
    assert!(host_commands.try_recv().is_err());
}

#[test]
fn sound_controller_load_failures_return_the_existing_reason_without_dispatch_or_file_write() {
    for failure in [
        EffetuneFailure::LoadFailed("component initialize failed: fake".into()),
        EffetuneFailure::LoadFailed("fake loader failure".into()),
        EffetuneFailure::RestoreFailed("fake restore failure".into()),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mixwright-state.json");
        let (mut controller, host_commands) = loading_controller(&path);
        let expected = failure.user_reason().to_owned();
        let ticket = controller.submit_sound(
            SoundRequest::Lock,
            "test client",
            ChangeController::new(),
            0.0,
            None,
        );
        controller.set_test_startup_completion(Err(failure.clone()));
        assert!(controller.poll());
        assert_eq!(controller.runtime, EffetuneRuntime::Failed(failure));
        assert_eq!(ticket.wait(), Err(SoundError::Unavailable(expected)));
        assert!(controller.sound_pending_load.is_empty());
        assert!(controller.bridge().is_none());
        assert!(host_commands.try_recv().is_err());
        assert!(!path.exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[test]
fn sound_controller_unavailable_load_completion_rejects_all_waiting_jobs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mixwright-state.json");
    let (mut controller, host_commands) = loading_controller(&path);
    let changes = ChangeController::new();
    let first = controller.submit_sound(
        SoundRequest::Lock,
        "first client",
        Arc::clone(&changes),
        0.0,
        None,
    );
    let second = controller.submit_sound(
        SoundRequest::Get(Default::default()),
        "second client",
        changes,
        0.0,
        None,
    );
    let reason = UnavailableReason::CpuUnsupported;
    let expected = reason.user_reason().to_owned();
    let (tx, rx) = mpsc::channel();
    controller.pending_load = Some(rx);
    tx.send(LoadCompletion::Unavailable(reason.clone()))
        .unwrap();
    assert!(controller.poll());
    assert_eq!(controller.runtime, EffetuneRuntime::Unavailable(reason));
    for ticket in [first, second] {
        assert_eq!(
            ticket.wait(),
            Err(SoundError::Unavailable(expected.clone()))
        );
    }
    assert!(controller.sound_pending_load.is_empty());
    assert!(host_commands.try_recv().is_err());
    assert!(!path.exists());
}

#[test]
fn sound_controller_preparation_failure_closes_all_waiters_without_automatic_reload() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mixwright-state.json");
    let (mut controller, host_commands) = loading_controller(&path);
    let changes = ChangeController::new();
    let first = controller.submit_sound(
        SoundRequest::Lock,
        "first client",
        Arc::clone(&changes),
        0.0,
        None,
    );
    let second = controller.submit_sound(
        SoundRequest::Get(Default::default()),
        "second client",
        Arc::clone(&changes),
        0.0,
        None,
    );
    let reason = UnavailableReason::BundlePreparationFailed {
        reason: "fake preparation failure".into(),
        rejected_generation: Some("invalid-test-generation".into()),
    };
    let expected = reason.user_reason().to_owned();
    let (tx, rx) = mpsc::channel();
    controller.pending_load = Some(rx);
    tx.send(LoadCompletion::Unavailable(reason.clone()))
        .unwrap();

    assert!(controller.poll());
    for ticket in [first, second] {
        assert_eq!(
            ticket.wait(),
            Err(SoundError::Unavailable(expected.clone()))
        );
    }
    assert_eq!(controller.runtime, EffetuneRuntime::Unavailable(reason));
    // Keep the existing permission for a later explicit request, while no
    // failed job may re-enter start_load on this or subsequent polls.
    assert!(controller.runtime.can_start_load());
    for _ in 0..3 {
        assert!(controller.sound_pending_load.is_empty());
        assert!(controller.pending_load.is_none());
        assert!(!controller.has_pending_ui_work());
        assert!(!controller.poll());
    }
    let records = changes.records();
    let records = records.lock().unwrap();
    assert_eq!(records.entries().len(), 2);
    assert!(
        records
            .entries()
            .iter()
            .all(|record| record.notice.contains(&expected))
    );
    assert!(controller.bridge().is_none());
    assert!(host_commands.try_recv().is_err());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn sound_controller_capture_and_sound_share_fifo_and_only_capture_worker_writes_the_pipe() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mixwright-state.json");
    let (mut controller, host_commands) = loading_controller(&path);
    let (host, commands, events) = Bridge::fake_host_for_test();
    let bridge = fake_dsp(directory.path());
    bridge.install_fake_host_for_test(Arc::clone(&host));
    controller.set_test_load_bridge_completion(Arc::clone(&bridge));
    assert!(controller.poll());
    assert!(matches!(
        host_commands.recv_timeout(WAIT).unwrap(),
        HostCommand::RegisterBridge(_)
    ));
    let first_capture = controller.captures.request(Arc::clone(&bridge)).unwrap();
    let ticket = controller.submit_sound(
        SoundRequest::Lock,
        "test client",
        ChangeController::new(),
        0.0,
        None,
    );
    let last_capture = controller.captures.request(Arc::clone(&bridge)).unwrap();

    let first = command(&commands, "query_state_concurrent");
    assert!(
        commands.try_recv().is_err(),
        "sound must wait behind the first capture"
    );
    let first_state = state("first capture");
    events.emit(Event::PluginState {
        request_id: first["request_id"].as_u64(),
        slot_id: 0,
        state: base64::engine::general_purpose::STANDARD.encode(&first_state),
        error: None,
    });
    assert_eq!(
        first_capture.recv_timeout(WAIT).unwrap().unwrap(),
        first_state
    );

    let lock = command(&commands, "sound_lock");
    assert!(
        commands.try_recv().is_err(),
        "last capture must wait for the sound job"
    );
    assert_eq!(fs::read(&path).unwrap(), first_state);
    events.emit(Event::SoundLock(reply(&lock, "Locked", None)));
    let persist = command(&commands, "sound_get");
    assert!(
        commands.try_recv().is_err(),
        "sound persistence is part of the same FIFO item"
    );
    let sound_state = state("sound persistence");
    events.emit(Event::SoundGet(reply(
        &persist,
        "State",
        Some(&sound_state),
    )));
    let response = ticket.wait().unwrap();
    assert!(matches!(response.outcome, SoundOutcome::Locked { .. }));
    assert!(response.persistence_warning.is_none());

    let last = command(&commands, "query_state_concurrent");
    assert_eq!(fs::read(&path).unwrap(), sound_state);
    let final_state = state("last capture");
    events.emit(Event::PluginState {
        request_id: last["request_id"].as_u64(),
        slot_id: 0,
        state: base64::engine::general_purpose::STANDARD.encode(&final_state),
        error: None,
    });
    assert_eq!(
        last_capture.recv_timeout(WAIT).unwrap().unwrap(),
        final_state
    );
    assert_eq!(fs::read(&path).unwrap(), final_state);
    let (generation, latest, effective) = controller.captures.latest_state().unwrap();
    assert_eq!(generation, 3);
    assert_eq!(latest, final_state);
    assert_eq!(effective, EffectiveState::Effective);
    assert_eq!(
        host.fake_command_threads_for_test(),
        vec!["effetune-state-capture"; 4]
    );
    assert!(commands.try_recv().is_err());
    assert!(host_commands.try_recv().is_err());
}

#[test]
fn sound_controller_stopped_capture_queue_aborts_sound_without_dispatch() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mixwright-state.json");
    let (mut controller, host_commands) = loading_controller(&path);
    let (host, commands, _events) = Bridge::fake_host_for_test();
    let bridge = fake_dsp(directory.path());
    bridge.install_fake_host_for_test(host);
    controller.set_test_load_bridge_completion(bridge);
    assert!(controller.poll());
    assert!(matches!(
        host_commands.recv_timeout(WAIT).unwrap(),
        HostCommand::RegisterBridge(_)
    ));
    controller
        .captures
        .accepting
        .store(false, Ordering::Release);
    let ticket = controller.submit_sound(
        SoundRequest::Lock,
        "test client",
        ChangeController::new(),
        0.0,
        None,
    );
    assert_eq!(ticket.wait(), Err(SoundError::Aborted));
    assert!(commands.try_recv().is_err());
    assert!(!path.exists());
}

#[test]
fn sound_controller_user_release_publishes_one_notice_and_record_after_poll() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mixwright-state.json");
    let (mut controller, host_commands) = loading_controller(&path);
    let (host, commands, events) = Bridge::fake_host_for_test();
    let bridge = fake_dsp(directory.path());
    bridge.install_fake_host_for_test(host);
    controller.set_test_load_bridge_completion(bridge);
    assert!(controller.poll());
    assert!(matches!(
        host_commands.recv_timeout(WAIT).unwrap(),
        HostCommand::RegisterBridge(_)
    ));
    let changes = ChangeController::new();
    let notices = changes.install(egui::Context::default());
    changes.set_ready(true);
    controller.sound_changes = Some(Arc::clone(&changes));

    // The fake pump follows the same typed event router as the real stdout
    // pump. Only polling the controller turns the release into UI history.
    events.emit(Event::SoundLock(SoundReply {
        request_id: 0,
        slot_id: 0,
        status: "Locked".into(),
        lock: Some("released-test-lock".into()),
        generation: 0,
        state: None,
        guard: None,
    }));
    assert!(controller.sound_locked());
    events.emit(Event::SoundLockReleased(SoundLockRelease {
        slot_id: 0,
        lock: "released-test-lock".into(),
        by: "user".into(),
    }));
    assert!(!controller.sound_locked());
    assert!(changes.records().lock().unwrap().entries().is_empty());
    assert!(notices.try_recv().is_err());
    controller.poll();
    assert_eq!(
        notices.recv_timeout(WAIT).unwrap(),
        "mIV Sound のロックを解除しました (利用者)"
    );
    {
        let records = changes.records();
        let records = records.lock().unwrap();
        assert_eq!(records.entries().len(), 1);
        let record = &records.entries()[0];
        assert_eq!(record.tool, "sound_chain");
        assert_eq!(record.client_name, "利用者");
        assert_eq!(record.counts.changed, 1);
        assert_eq!(record.rows.len(), 1);
        assert_eq!(record.rows[0].item, "mIV Sound のロック");
        assert_eq!(record.rows[0].before, "開始");
        assert_eq!(record.rows[0].after, "解除 (利用者)");
        assert_eq!(
            record.rows[0].result,
            ChangeRowResult::Done(ChangeRowOutcome::Changed)
        );
    }
    controller.poll();
    assert!(notices.try_recv().is_err());
    assert_eq!(changes.records().lock().unwrap().entries().len(), 1);
    assert!(
        commands.try_recv().is_err(),
        "release notification must not issue a host command"
    );
    assert!(host_commands.try_recv().is_err());
    assert!(!path.exists());
}
