use super::*;

fn reply(id: u64, status: &str) -> SoundReply {
    SoundReply {
        request_id: id,
        slot_id: 7,
        status: status.into(),
        lock: Some("token".into()),
        generation: 3,
        state: None,
        guard: Some(SoundPresetGuard::Epoch { epoch: 2 }),
    }
}

#[test]
fn sound_responses_follow_request_ids_without_entering_sync_events() {
    let (host, commands, events) = Bridge::fake_host_for_test();
    let first = host.sound_request(7, SoundCommand::Get).unwrap();
    let second = host.sound_request(7, SoundCommand::Lock).unwrap();
    let a = commands.recv().unwrap();
    let b = commands.recv().unwrap();
    assert_eq!(a["cmd"], "sound_get");
    assert_eq!(a["slot_id"], 7);
    events.emit(Event::SoundLock(reply(
        b["request_id"].as_u64().unwrap(),
        "Locked",
    )));
    assert_eq!(second.recv().unwrap().unwrap().status, "Locked");
    assert!(first.try_recv().is_err());
    events.emit(Event::SoundGet(reply(99_999, "State")));
    assert!(first.try_recv().is_err());
    events.emit(Event::SoundGet(reply(
        a["request_id"].as_u64().unwrap(),
        "State",
    )));
    assert_eq!(
        first.recv().unwrap().unwrap().revision().as_deref(),
        Some("token:3")
    );
    assert!(host.event_rx.try_recv().is_err());
    assert!(host.pending_sound_requests.lock().unwrap().is_empty());
}

#[test]
fn sound_transport_shutdown_aborts_all_requests_and_future_admission() {
    let (host, _commands, _events) = Bridge::fake_host_for_test();
    let first = host.sound_request(7, SoundCommand::Get).unwrap();
    let second = host
        .sound_request(
            7,
            SoundCommand::Undo {
                lock: "token".into(),
            },
        )
        .unwrap();
    host.abort_state_queries();
    assert_eq!(first.recv().unwrap(), Err(ConcurrentStateError::HostExited));
    assert_eq!(
        second.recv().unwrap(),
        Err(ConcurrentStateError::HostExited)
    );
    assert!(matches!(
        host.sound_request(7, SoundCommand::Lock),
        Err(ConcurrentStateError::HostExited)
    ));
}

#[test]
fn sound_user_release_has_a_separate_notice_and_clears_lock_projection() {
    let (host, commands, events) = Bridge::fake_host_for_test();
    let pending = host.sound_request(7, SoundCommand::Lock).unwrap();
    let id = commands.recv().unwrap()["request_id"].as_u64().unwrap();
    events.emit(Event::SoundLock(reply(id, "Locked")));
    pending.recv().unwrap().unwrap();
    assert!(host.sound_locked());
    let release = SoundLockRelease {
        slot_id: 7,
        lock: "token".into(),
        by: "user".into(),
    };
    events.emit(Event::SoundLockReleased(release.clone()));
    assert!(!host.sound_locked());
    assert_eq!(host.poll_sound_release(), Some(release));
    assert!(host.event_rx.try_recv().is_err());
}

#[test]
fn sound_command_and_guard_wire_shapes_match_protocol_nine() {
    assert_eq!(PROTOCOL_VERSION, 9);
    assert!(require_current_protocol(8).is_err());
    assert!(require_current_protocol(9).is_ok());
    let wire = SoundCommand::PresetCommit {
        lock: "x".into(),
        generation: 4,
        guard: SoundPresetGuard::Epoch { epoch: 6 },
    }
    .wire(7, 8);
    assert_eq!(
        wire,
        serde_json::json!({"cmd":"sound_preset_commit", "lock":"x",
        "generation":4,"guard":{"epoch":6},"slot_id":7,"request_id":8})
    );
    let absent = SoundCommand::PresetCommit {
        lock: "x".into(),
        generation: 4,
        guard: SoundPresetGuard::Absent("absent".into()),
    }
    .wire(0, 9);
    assert_eq!(absent["guard"], "absent");
    for command in [
        SoundCommand::Lock,
        SoundCommand::Get,
        SoundCommand::Unlock { lock: "x".into() },
        SoundCommand::Undo { lock: "x".into() },
        SoundCommand::Replace {
            lock: "x".into(),
            generation: 2,
            state: "e30=".into(),
        },
    ] {
        let wire = command.wire(7, 19);
        assert_eq!(wire["request_id"], 19);
        assert_eq!(wire["slot_id"], 7);
    }
}
