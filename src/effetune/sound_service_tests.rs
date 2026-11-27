use super::*;
use crate::video::dsp::bridge::SoundPresetGuard;
use std::{cell::Cell, fs, sync::Mutex};

struct FakeHost<F> {
    commands: Mutex<Vec<SoundCommand>>,
    respond: F,
}

impl<F> FakeHost<F>
where
    F: Fn(SoundCommand) -> Result<SoundReply, SoundError>,
{
    fn new(respond: F) -> Self {
        Self {
            commands: Mutex::new(Vec::new()),
            respond,
        }
    }

    fn only_get(&self) {
        let commands = self.commands.lock().unwrap();
        assert_eq!(commands.len(), 1);
        assert!(matches!(commands[0], SoundCommand::Get));
    }
}

impl<F> SoundHost for FakeHost<F>
where
    F: Fn(SoundCommand) -> Result<SoundReply, SoundError>,
{
    fn call(&self, command: SoundCommand) -> Result<SoundReply, SoundError> {
        self.commands.lock().unwrap().push(command.clone());
        (self.respond)(command)
    }

    fn latency_secs(&self) -> f64 {
        0.125
    }
}

/// Return an owned job separately from its response ticket, just as the FIFO
/// does. Dropping the unstarted closure exercises queue shutdown as well.
pub(crate) fn sound_job_with_fake_host(
    request: SoundRequest,
    client: String,
    changes: Arc<ChangeController>,
    permit: Option<Arc<crate::agent_ipc::Permit>>,
) -> (
    SoundTicket,
    mpsc::Receiver<SoundCommand>,
    mpsc::Sender<Result<SoundReply, SoundError>>,
    impl FnOnce() -> bool + Send + 'static,
) {
    let (mut job, ticket) = job(request, &client, changes, 0.0);
    job.permit = permit;
    let (commands, command_rx) = mpsc::channel();
    let (replies, reply_rx) = mpsc::channel();
    let run = move || {
        let directory = tempfile::tempdir().unwrap();
        let host = FakeHost::new(move |command| {
            commands.send(command).map_err(|_| SoundError::Aborted)?;
            reply_rx.recv().unwrap_or(Err(SoundError::Aborted))
        });
        let mut saved = false;
        run_with_host(job, &host, directory.path(), |_| {
            saved = true;
            Ok(())
        });
        saved
    };
    (ticket, command_rx, replies, run)
}

fn state(bypass: bool) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "formatVersion":1,"appVersion":"test","pipelineA":[
            {"id":7,"type":"VolumePlugin","name":"Original","enabled":true,
                "parameters":{"vl":-3,"_uiCollapsed":true}}
        ],"pipelineB":null,"currentPipeline":"A","masterBypass":bypass,
        "oversampling":1,"automation":{"logicalPluginIdWatermark":7,"bindings":[]},
        "ui":{"unknown":"keep"}
    }))
    .unwrap()
}

fn reply(status: &str, generation: u64) -> SoundReply {
    SoundReply {
        request_id: 1,
        slot_id: 0,
        status: status.into(),
        lock: Some("test-lock".into()),
        generation,
        state: None,
        guard: None,
    }
}

fn state_reply(generation: u64, guard: Option<SoundPresetGuard>, bypass: bool) -> SoundReply {
    SoundReply {
        state: Some(base64::engine::general_purpose::STANDARD.encode(state(bypass))),
        guard,
        ..reply("State", generation)
    }
}

fn save_request() -> SoundRequest {
    SoundRequest::SavePreset {
        lock: "test-lock".into(),
        pipeline: Pipeline::A,
        name: "test preset".into(),
        replace: false,
    }
}

fn select_request(revision: &str) -> SoundRequest {
    SoundRequest::Select {
        lock: "test-lock".into(),
        revision: revision.into(),
        input: SelectInput {
            use_pipeline: None,
            bypass: Some(true),
        },
    }
}

#[test]
fn sound_service_revision_and_lock_mismatch_prevent_replace_for_both_edit_routes() {
    let directory = tempfile::tempdir().unwrap();
    let apply = SoundRequest::Apply {
        lock: "test-lock".into(),
        revision: "test-lock:2".into(),
        input: serde_json::from_value(json!({"pipeline":"B","effects":[]})).unwrap(),
    };
    for request in [apply, select_request("test-lock:2")] {
        for unlocked in [false, true] {
            let host = FakeHost::new(|command| {
                assert!(matches!(command, SoundCommand::Get));
                let mut current = state_reply(3, None, false);
                if unlocked {
                    current.lock = Some("new-lock".into());
                }
                Ok(current)
            });
            match execute(&host, &request, directory.path(), 0.0) {
                Err(error) => assert_eq!(
                    error,
                    if unlocked {
                        SoundError::Unlocked
                    } else {
                        SoundError::Changed
                    }
                ),
                Ok(_) => panic!("stale revision or released lock must not mutate state"),
            }
            host.only_get();
        }
    }
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn sound_service_absent_preset_uses_fixed_closed_temp_before_commit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    let preset_path = path.join("effetune_presets.json");
    let temp_path = path.join("effetune_presets.json.miv-mcp.tmp");
    let host = FakeHost::new(|command| match command {
        SoundCommand::Get => Ok(state_reply(
            3,
            Some(SoundPresetGuard::Absent("absent".into())),
            false,
        )),
        SoundCommand::PresetCommit {
            lock,
            generation,
            guard,
        } => {
            assert_eq!(lock, "test-lock");
            assert_eq!(generation, 3);
            assert_eq!(guard, SoundPresetGuard::Absent("absent".into()));
            assert!(!preset_path.exists());
            assert!(temp_path.is_file());
            assert_eq!(
                fs::read_dir(path).unwrap().count(),
                1,
                "only the fixed temp is created"
            );
            let mut options = fs::OpenOptions::new();
            options.read(true);
            // Windows denies this open if the worker still holds its write
            // handle, even if that original handle allowed sharing.
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(0);
            }
            let mut file = options
                .open(&temp_path)
                .expect("temp handle closed before host commit");
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            let presets: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(presets.as_object().unwrap().len(), 1);
            assert_eq!(
                presets["MCP: test preset"]["pipeline"],
                serde_json::from_slice::<Value>(&state(false)).unwrap()["pipelineA"]
            );
            drop(file);
            fs::rename(&temp_path, &preset_path).unwrap();
            Ok(reply("Committed", 3))
        }
        other => panic!("unexpected command: {other:?}"),
    });
    let result =
        execute(&host, &save_request(), path, 0.0).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        matches!(result.outcome, SoundOutcome::PresetSaved { replaced: false, ref name, .. } if name == "MCP: test preset")
    );
    assert!(preset_path.is_file());
    assert!(!temp_path.exists());
    assert_eq!(result.rows.len(), 1);
    assert_eq!(host.commands.lock().unwrap().len(), 2);
}

#[test]
fn sound_service_corrupt_or_oversized_preset_never_creates_temp_or_commits() {
    let directory = tempfile::tempdir().unwrap();
    let preset_path = directory.path().join("effetune_presets.json");
    let mut near_limit = serde_json::to_vec(
        &json!({"User preset":{"padding":"x".repeat(PRESET_LIMIT as usize - 96)}}),
    )
    .unwrap();
    // Ensure the input itself fits while the newly composed whole file does not.
    assert!((near_limit.len() as u64) < PRESET_LIMIT);
    assert!(PRESET_LIMIT - near_limit.len() as u64 <= 96);
    for existing in [
        b"{broken".to_vec(),
        b"[]".to_vec(),
        vec![b' '; PRESET_LIMIT as usize + 1],
        std::mem::take(&mut near_limit),
    ] {
        fs::write(&preset_path, &existing).unwrap();
        let host = FakeHost::new(|command| {
            assert!(matches!(command, SoundCommand::Get));
            Ok(state_reply(
                3,
                Some(SoundPresetGuard::Epoch { epoch: 4 }),
                false,
            ))
        });
        assert!(matches!(
            execute(&host, &save_request(), directory.path(), 0.0),
            Err(SoundError::Invalid(_))
        ));
        host.only_get();
        assert_eq!(
            fs::read(&preset_path).unwrap(),
            existing,
            "original stays byte-for-byte intact"
        );
        assert!(
            !directory
                .path()
                .join("effetune_presets.json.miv-mcp.tmp")
                .exists()
        );
    }
}

#[test]
fn sound_service_missing_preset_guard_is_changed_without_any_file_write_or_commit() {
    let directory = tempfile::tempdir().unwrap();
    let host = FakeHost::new(|command| {
        assert!(matches!(command, SoundCommand::Get));
        Ok(state_reply(3, None, false))
    });
    assert!(matches!(
        execute(&host, &save_request(), directory.path(), 0.0),
        Err(SoundError::Changed)
    ));
    host.only_get();
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn sound_service_changed_preset_commit_keeps_original_and_publishes_no_success() {
    let directory = tempfile::tempdir().unwrap();
    let preset_path = directory.path().join("effetune_presets.json");
    let original = br#"{"User preset":{"pipeline":[]}}"#;
    fs::write(&preset_path, original).unwrap();
    let host = FakeHost::new(|command| match command {
        SoundCommand::Get => Ok(state_reply(
            3,
            Some(SoundPresetGuard::Epoch { epoch: 4 }),
            false,
        )),
        SoundCommand::PresetCommit { guard, .. } => {
            assert_eq!(guard, SoundPresetGuard::Epoch { epoch: 4 });
            assert!(
                directory
                    .path()
                    .join("effetune_presets.json.miv-mcp.tmp")
                    .is_file()
            );
            Ok(reply("Changed", 3))
        }
        other => panic!("unexpected command: {other:?}"),
    });
    let changes = ChangeController::new();
    let (job, ticket) = job(save_request(), "test client", Arc::clone(&changes), 0.0);
    run_with_host(job, &host, directory.path(), |_| {
        panic!("failed commit must not persist")
    });
    assert_eq!(ticket.wait(), Err(SoundError::Changed));
    assert_eq!(fs::read(&preset_path).unwrap(), original);
    let records = changes.records();
    let records = records.lock().unwrap();
    assert_eq!(records.entries().len(), 1);
    assert_eq!(records.entries()[0].counts.changed, 0);
    assert!(records.entries()[0].rows.is_empty());
    assert_eq!(host.commands.lock().unwrap().len(), 2);
}

#[test]
fn sound_service_expired_ticket_does_not_cancel_host_receiver_persistence_or_notice() {
    let directory = tempfile::tempdir().unwrap();
    let changes = ChangeController::new();
    let notices = changes.install(egui::Context::default());
    let (job, mut ticket) = job(
        select_request("test-lock:3"),
        "late client",
        Arc::clone(&changes),
        0.0,
    );
    ticket.deadline = Instant::now() - Duration::from_secs(1);
    let (entered, entered_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let (persisted, persisted_rx) = mpsc::channel();
    let path = directory.path().to_owned();
    let worker = std::thread::spawn(move || {
        let gets = Cell::new(0);
        let host = FakeHost::new(|command| match command {
            SoundCommand::Get => {
                let count = gets.get();
                gets.set(count + 1);
                if count == 0 {
                    entered.send(()).unwrap();
                    release_rx
                        .recv_timeout(Duration::from_secs(5))
                        .expect("release fake host result");
                }
                Ok(state_reply(
                    if count == 0 { 3 } else { 4 },
                    None,
                    count != 0,
                ))
            }
            SoundCommand::Replace {
                lock,
                generation,
                state: replacement,
            } => {
                assert_eq!(lock, "test-lock");
                assert_eq!(generation, 3);
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(replacement)
                    .unwrap();
                assert_eq!(
                    serde_json::from_slice::<Value>(&bytes).unwrap()["masterBypass"],
                    true
                );
                Ok(reply("Replaced", 4))
            }
            other => panic!("unexpected command: {other:?}"),
        });
        run_with_host(job, &host, &path, |bytes| {
            persisted.send(bytes.to_vec()).unwrap();
            Ok(())
        });
        host.commands.into_inner().unwrap()
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(
        ticket.wait().unwrap().outcome,
        SoundOutcome::Pending { .. }
    ));
    assert!(matches!(
        persisted_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(changes.records().lock().unwrap().entries().is_empty());
    // The caller's receiver has now been dropped. The blocked worker's host
    // receiver remains alive and accepts the late result without a real sleep.
    release.send(()).unwrap();
    assert_eq!(
        persisted_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        state(true)
    );
    let commands = worker.join().unwrap();
    assert_eq!(commands.len(), 3);
    assert!(matches!(commands[0], SoundCommand::Get));
    assert!(matches!(commands[1], SoundCommand::Replace { .. }));
    assert!(matches!(commands[2], SoundCommand::Get));
    let notice = notices.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(notice.contains("オフ"));
    let records = changes.records();
    let records = records.lock().unwrap();
    assert_eq!(records.entries().len(), 1);
    assert_eq!(records.entries()[0].client_name, "late client");
    assert_eq!(records.entries()[0].counts.changed, 1);
}

#[test]
fn sound_service_host_abort_finishes_ticket_and_records_failure_without_persisting() {
    let directory = tempfile::tempdir().unwrap();
    let host = FakeHost::new(|command| {
        assert!(matches!(command, SoundCommand::Get));
        Err(SoundError::Aborted)
    });
    let changes = ChangeController::new();
    let (job, ticket) = job(
        SoundRequest::Get(GetInput::default()),
        "test client",
        Arc::clone(&changes),
        0.0,
    );
    run_with_host(job, &host, directory.path(), |_| {
        panic!("aborted request must not persist")
    });
    assert_eq!(ticket.wait(), Err(SoundError::Aborted));
    host.only_get();
    let records = changes.records();
    let records = records.lock().unwrap();
    assert_eq!(records.entries().len(), 1);
    assert!(records.entries()[0].notice.contains("中断"));
    assert_eq!(records.entries()[0].counts.changed, 0);
}

#[test]
fn sound_service_get_persists_fresh_snapshot_without_publishing_change() {
    let directory = tempfile::tempdir().unwrap();
    let gets = Cell::new(0);
    let host = FakeHost::new(|command| {
        assert!(matches!(command, SoundCommand::Get));
        let count = gets.get();
        gets.set(count + 1);
        Ok(state_reply(
            if count == 0 { 3 } else { 4 },
            None,
            count != 0,
        ))
    });
    let changes = ChangeController::new();
    let (job, ticket) = job(
        SoundRequest::Get(GetInput::default()),
        "test client",
        Arc::clone(&changes),
        0.0,
    );
    let mut saved = None;
    run_with_host(job, &host, directory.path(), |bytes| {
        saved = Some(bytes.to_vec());
        Ok(())
    });
    let response = ticket.wait().unwrap();
    assert!(
        matches!(response.outcome, SoundOutcome::State { ref state } if state["revision"] == "test-lock:3")
    );
    assert_eq!(saved, Some(state(true)));
    assert_eq!(gets.get(), 2);
    assert!(changes.records().lock().unwrap().entries().is_empty());
    assert_eq!(response.persistence_warning, None);
}

#[test]
fn sound_service_unlock_released_and_already_released_persist_with_only_real_release_notice() {
    for already_released in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let host = FakeHost::new(|command| match command {
            SoundCommand::Unlock { lock } => {
                assert_eq!(lock, "test-lock");
                let mut result = reply(
                    if already_released {
                        "AlreadyReleased"
                    } else {
                        "Released"
                    },
                    0,
                );
                result.lock = None;
                Ok(result)
            }
            SoundCommand::Get => {
                let mut result = state_reply(0, None, false);
                result.lock = None;
                Ok(result)
            }
            other => panic!("unexpected command: {other:?}"),
        });
        let changes = ChangeController::new();
        let notices = changes.install(egui::Context::default());
        let (job, ticket) = job(
            SoundRequest::Unlock {
                lock: "test-lock".into(),
            },
            "unlock client",
            Arc::clone(&changes),
            0.0,
        );
        let mut saved = None;
        run_with_host(job, &host, directory.path(), |bytes| {
            saved = Some(bytes.to_vec());
            Ok(())
        });
        let response = ticket.wait().unwrap();
        assert_eq!(
            response.outcome,
            SoundOutcome::Released { already_released }
        );
        assert_eq!(response.persistence_warning, None);
        assert_eq!(saved, Some(state(false)));
        let commands = host.commands.lock().unwrap();
        assert_eq!(commands.len(), 2);
        assert!(matches!(&commands[0], SoundCommand::Unlock { .. }));
        assert!(matches!(&commands[1], SoundCommand::Get));
        let records = changes.records();
        let records = records.lock().unwrap();
        if already_released {
            assert!(notices.try_recv().is_err());
            assert!(records.entries().is_empty());
        } else {
            assert_eq!(
                notices.try_recv().unwrap(),
                "AI アプリが mIV Sound のロックを解除しました"
            );
            assert!(notices.try_recv().is_err());
            assert_eq!(records.entries().len(), 1);
            let record = &records.entries()[0];
            assert_eq!(record.client_name, "unlock client");
            assert_eq!(record.counts.changed, 1);
            assert_eq!(record.rows.len(), 1);
            assert_eq!(record.rows[0].item, "mIV Sound のロック");
            assert_eq!(record.rows[0].before, "開始");
            assert_eq!(record.rows[0].after, "解除 (AI)");
        }
    }
}

#[test]
fn sound_service_undo_success_persists_and_publishes_one_notice_and_record() {
    let directory = tempfile::tempdir().unwrap();
    let host = FakeHost::new(|command| match command {
        SoundCommand::Undo { lock } => {
            assert_eq!(lock, "test-lock");
            Ok(reply("Undone", 4))
        }
        SoundCommand::Get => Ok(state_reply(4, None, true)),
        other => panic!("unexpected command: {other:?}"),
    });
    let changes = ChangeController::new();
    let notices = changes.install(egui::Context::default());
    let (job, ticket) = job(
        SoundRequest::Undo {
            lock: "test-lock".into(),
        },
        "undo client",
        Arc::clone(&changes),
        0.0,
    );
    let mut saved = None;
    run_with_host(job, &host, directory.path(), |bytes| {
        saved = Some(bytes.to_vec());
        Ok(())
    });
    let response = ticket.wait().unwrap();
    assert_eq!(
        response.outcome,
        SoundOutcome::Undone {
            revision: "test-lock:4".into()
        }
    );
    assert_eq!(response.persistence_warning, None);
    assert_eq!(saved, Some(state(true)));
    let commands = host.commands.lock().unwrap();
    assert_eq!(commands.len(), 2);
    assert!(matches!(&commands[0], SoundCommand::Undo { .. }));
    assert!(matches!(&commands[1], SoundCommand::Get));
    assert_eq!(
        notices.try_recv().unwrap(),
        "AI アプリが mIV Sound の変更を取り消しました"
    );
    assert!(notices.try_recv().is_err());
    let records = changes.records();
    let records = records.lock().unwrap();
    assert_eq!(records.entries().len(), 1);
    let record = &records.entries()[0];
    assert_eq!(record.client_name, "undo client");
    assert_eq!(record.counts.changed, 1);
    assert_eq!(record.rows.len(), 1);
    assert_eq!(record.rows[0].item, "mIV Sound");
    assert_eq!(record.rows[0].before, "AI による変更");
    assert_eq!(record.rows[0].after, "取り消し");
}

#[test]
fn sound_service_undo_nothing_or_unlocked_never_persists_or_records_success() {
    for (status, expected) in [
        ("NothingToUndo", SoundError::NothingToUndo),
        ("Unlocked", SoundError::Unlocked),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let host = FakeHost::new(|command| {
            let SoundCommand::Undo { lock } = command else {
                panic!("rejected undo must not fetch state");
            };
            assert_eq!(lock, "test-lock");
            Ok(reply(status, 3))
        });
        let changes = ChangeController::new();
        let notices = changes.install(egui::Context::default());
        let (job, ticket) = job(
            SoundRequest::Undo {
                lock: "test-lock".into(),
            },
            "undo client",
            Arc::clone(&changes),
            0.0,
        );
        run_with_host(job, &host, directory.path(), |_| {
            panic!("rejected undo must not persist")
        });
        let expected_notice = format!("mIV Sound: {expected}");
        assert_eq!(ticket.wait(), Err(expected));
        assert_eq!(host.commands.lock().unwrap().len(), 1);
        // Failures still have their existing notice/history entry, including
        // late results, but never a successful undo row or state save.
        assert_eq!(notices.try_recv().unwrap(), expected_notice);
        assert!(notices.try_recv().is_err());
        let records = changes.records();
        let records = records.lock().unwrap();
        assert_eq!(records.entries().len(), 1);
        assert_eq!(records.entries()[0].notice, expected_notice);
        assert_eq!(records.entries()[0].counts.changed, 0);
        assert!(records.entries()[0].rows.is_empty());
    }
}

#[test]
fn sound_service_parent_wait_budget_does_not_extend_or_cancel_the_accepted_job() {
    let directory = tempfile::tempdir().unwrap();
    let changes = ChangeController::new();
    let (job, ticket) = job(
        SoundRequest::Lock,
        "deadline client",
        Arc::clone(&changes),
        0.0,
    );
    let own_deadline = ticket.deadline;
    let ticket = ticket.with_wait_budget(Duration::from_secs(60));
    assert_eq!(ticket.deadline, own_deadline);
    let ticket = ticket.with_wait_budget(Duration::ZERO);
    assert!(ticket.deadline <= Instant::now());
    assert!(matches!(
        ticket.wait().unwrap().outcome,
        SoundOutcome::Pending { .. }
    ));
    let host = FakeHost::new(|command| match command {
        SoundCommand::Lock => Ok(reply("Locked", 0)),
        SoundCommand::Get => Ok(state_reply(0, None, false)),
        other => panic!("unexpected command: {other:?}"),
    });
    let mut saved = false;
    run_with_host(job, &host, directory.path(), |_| {
        saved = true;
        Ok(())
    });
    assert!(saved);
    assert_eq!(host.commands.lock().unwrap().len(), 2);
    assert_eq!(changes.records().lock().unwrap().entries().len(), 1);
}

#[test]
fn sound_service_reject_and_disconnected_queue_return_aborted_without_host() {
    let changes = ChangeController::new();
    let (queued, ticket) = job(SoundRequest::Lock, "test client", Arc::clone(&changes), 0.0);
    reject(queued, SoundError::Aborted);
    assert_eq!(ticket.wait(), Err(SoundError::Aborted));
    let (queued, ticket) = job(SoundRequest::Lock, "test client", changes, 0.0);
    drop(queued);
    assert_eq!(ticket.wait(), Err(SoundError::Aborted));
    assert_eq!(
        bridge_error(ConcurrentStateError::HostExited),
        SoundError::Aborted
    );
    assert_eq!(
        bridge_error(ConcurrentStateError::Interrupted("test".into())),
        SoundError::Aborted
    );
}
