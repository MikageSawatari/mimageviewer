use super::tests::Fixture;
use super::*;
use crate::collection_store::{CollectionId, CollectionSnapshot, CollectionStoreClient};

fn ready(fixture: &Fixture) -> changes::UiReceiver {
    fixture.set_level(AgentLevel::Organize);
    let notices = fixture.handlers.changes.install(egui::Context::default());
    fixture.handlers.changes.set_ready(true);
    notices
}

fn actor(fixture: &Fixture) -> CollectionStoreClient {
    fixture.runtime.as_ref().unwrap().client()
}

fn snapshot(fixture: &Fixture, id: &str) -> CollectionSnapshot {
    actor(fixture)
        .load_collection(CollectionId::from_uuid(uuid::Uuid::parse_str(id).unwrap()))
        .unwrap()
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap()
}

fn create(fixture: &Fixture, connection: &Arc<Connection>, name: &str) -> String {
    let response = fixture.call(
        connection,
        101,
        Command::CreateCollection { name: name.into() },
    );
    match response.result.unwrap() {
        Payload::CollectionWrite(CollectionWriteResult::Created { id }) => id,
        result => panic!("expected created collection, got {result:?}"),
    }
}

fn item(path: &Path) -> ItemRef {
    ItemRef {
        path: path.to_string_lossy().into_owned(),
        subresource: None,
    }
}

fn outcomes(response: Response, expected_id: &str) -> Vec<CollectionItemOutcome> {
    match response.result.unwrap() {
        Payload::CollectionWrite(CollectionWriteResult::Items { id, outcomes }) => {
            assert_eq!(id, expected_id);
            let mut positions = outcomes
                .iter()
                .flat_map(|outcome| match outcome {
                    CollectionItemOutcome::Added { indices }
                    | CollectionItemOutcome::Duplicate { indices }
                    | CollectionItemOutcome::Removed { indices }
                    | CollectionItemOutcome::NotPresent { indices }
                    | CollectionItemOutcome::CapacityRejected { indices }
                    | CollectionItemOutcome::Skipped { indices, .. } => indices.iter().copied(),
                })
                .collect::<Vec<_>>();
            positions.sort_unstable();
            assert_eq!(positions, (0..positions.len() as u16).collect::<Vec<_>>());
            outcomes
        }
        result => panic!("expected collection item results, got {result:?}"),
    }
}

fn start(
    connection: &Arc<Connection>,
    command: Command,
) -> (JoinHandle<()>, crossbeam_channel::Receiver<Reply>) {
    let (send, receive) = crossbeam_channel::unbounded();
    let worker = connection
        .request(Request { id: 201, command }, send)
        .unwrap();
    (worker, receive)
}

fn clock(fixture: &mut Fixture) -> Arc<AtomicUsize> {
    let seconds = Arc::new(AtomicUsize::new(0));
    let clock = seconds.clone();
    let base = Instant::now();
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(move || base + Duration::from_secs(clock.load(Ordering::Acquire) as u64)),
        execute: Box::new(|_, _| panic!("collection writes must not use the read executor")),
    }));
    seconds
}

#[test]
fn create_and_rename_use_ids_allow_duplicate_names_and_record_actual_values() {
    let fixture = Fixture::new();
    let notices = ready(&fixture);
    let connection = fixture.connection();
    let first = create(&fixture, &connection, "same name");
    let second = create(&fixture, &connection, "same name");
    assert_ne!(first, second);
    let before = snapshot(&fixture, &first);
    let renamed = fixture.call(
        &connection,
        102,
        Command::RenameCollection {
            id: first.clone(),
            name: "new name".into(),
        },
    );
    assert_eq!(
        renamed.result.unwrap(),
        Payload::CollectionWrite(CollectionWriteResult::Renamed {
            id: first.clone(),
            changed: true,
        })
    );
    let after = snapshot(&fixture, &first);
    assert_eq!(after.definition.name, "new name");
    assert!(after.revision() > before.revision());
    assert_eq!(snapshot(&fixture, &second).definition.name, "same name");
    let unchanged = fixture.call(
        &connection,
        103,
        Command::RenameCollection {
            id: first.clone(),
            name: "new name".into(),
        },
    );
    assert_eq!(
        unchanged.result.unwrap(),
        Payload::CollectionWrite(CollectionWriteResult::Renamed {
            id: first.clone(),
            changed: false,
        })
    );
    assert_eq!(snapshot(&fixture, &first), after);
    let shared = fixture.handlers.changes.records();
    let records = shared.lock().unwrap();
    assert_eq!(records.entries().len(), 4);
    assert!(
        records
            .entries()
            .iter()
            .all(|record| record.client_name == "test")
    );
    let rename = &records.entries()[1];
    assert_eq!(rename.tool, "rename_collection");
    assert_eq!(rename.counts.changed, 1);
    assert_eq!(rename.rows.len(), 1);
    assert_eq!(rename.rows[0].before, "same name");
    assert_eq!(rename.rows[0].after, "new name");
    let created = &records.entries()[3];
    assert_eq!(created.tool, "create_collection");
    assert!(
        created
            .rows
            .iter()
            .any(|row| row.after.contains("same name")
                && (row.after.contains(&first) || row.item.contains(&first)))
    );
    drop(records);
    assert_eq!(notices.try_iter().count(), 4);
}

#[test]
fn uninstalled_starting_and_insufficient_permission_never_write_or_notify() {
    let fixture = Fixture::new();
    let connection = fixture.connection();
    let command = || Command::CreateCollection {
        name: "must not exist".into(),
    };
    assert_eq!(
        fixture
            .call(&connection, 101, command())
            .result
            .unwrap_err()
            .code,
        ErrorCode::Disabled
    );
    fixture.set_level(AgentLevel::ReadAndOpen);
    assert_eq!(
        fixture
            .call(&connection, 102, command())
            .result
            .unwrap_err()
            .code,
        ErrorCode::Disabled
    );
    fixture.set_level(AgentLevel::Organize);
    assert_eq!(
        fixture
            .call(&connection, 103, command())
            .result
            .unwrap_err()
            .code,
        ErrorCode::NotReady
    );
    let notices = fixture.handlers.changes.install(egui::Context::default());
    assert_eq!(
        fixture
            .call(&connection, 104, command())
            .result
            .unwrap_err()
            .code,
        ErrorCode::NotReady
    );
    assert!(
        actor(&fixture)
            .list_catalog()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap()
            .definitions
            .is_empty()
    );
    assert!(
        fixture
            .handlers
            .changes
            .records()
            .lock()
            .unwrap()
            .entries()
            .is_empty()
    );
    assert!(notices.try_recv().is_err());
}

#[test]
fn all_remote_phases_reject_without_mutating_remote_or_collection_state() {
    use crate::remote_ipc::session::{RemoteControlPhase, SessionHandle};
    use mimageviewer_ipc::{SessionAcquireRequest, SessionConnectionKind, SessionPeerInfo};
    for phase in [
        RemoteControlPhase::AcquiringRemote,
        RemoteControlPhase::RemoteActive,
        RemoteControlPhase::DrainingRemote,
    ] {
        let mut fixture = Fixture::new();
        let session = SessionHandle::new();
        session.acquire(SessionAcquireRequest {
            client_id: "collection-write-test".into(),
            peer: SessionPeerInfo {
                connection_kind: SessionConnectionKind::Direct,
                device_name: None,
            },
        });
        let generation = session.snapshot().generation;
        match phase {
            RemoteControlPhase::RemoteActive => assert!(session.finish_acquire(generation)),
            RemoteControlPhase::DrainingRemote => {
                assert!(session.abort_acquire_barrier(generation))
            }
            _ => {}
        }
        Arc::get_mut(&mut fixture.handlers).unwrap().session = Some(session.clone());
        let notices = ready(&fixture);
        let connection = fixture.connection();
        assert_eq!(
            fixture
                .call(
                    &connection,
                    101,
                    Command::CreateCollection {
                        name: "blocked".into()
                    }
                )
                .result
                .unwrap_err()
                .code,
            ErrorCode::Busy
        );
        assert_eq!(session.snapshot().phase, phase);
        assert_eq!(session.snapshot().generation, generation);
        assert!(
            actor(&fixture)
                .list_catalog()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap()
                .definitions
                .is_empty()
        );
        assert!(
            fixture
                .handlers
                .changes
                .records()
                .lock()
                .unwrap()
                .entries()
                .is_empty()
        );
        assert!(notices.try_recv().is_err());
    }
}

#[test]
fn revision_conflict_returns_error_without_retry_or_overwriting_user_rename() {
    let mut fixture = Fixture::new();
    let notices = ready(&fixture);
    let client = actor(&fixture);
    let seeded = client
        .create_collection("original".into())
        .unwrap()
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    let collection_id = seeded.collection_id();
    Arc::get_mut(&mut fixture.handlers).unwrap().write_hooks =
        Some(Arc::new(collection_write::WriteHooks {
            before_enqueue: Some(Arc::new(move || {
                client
                    .rename_collection(collection_id, seeded.revision(), "user change".into())
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap()
                    .unwrap();
            })),
            after_enqueue: None,
        }));
    let connection = fixture.connection();
    let response = fixture.call(
        &connection,
        101,
        Command::RenameCollection {
            id: collection_id.to_string(),
            name: "agent change".into(),
        },
    );
    assert_eq!(response.result.unwrap_err().code, ErrorCode::Conflict);
    assert_eq!(
        snapshot(&fixture, &collection_id.to_string())
            .definition
            .name,
        "user change"
    );
    let records = fixture.handlers.changes.records();
    let records = records.lock().unwrap();
    assert_eq!(records.entries().len(), 1);
    assert_eq!(records.entries()[0].counts.failed, 1);
    assert_eq!(notices.try_iter().count(), 1);
}

#[test]
fn full_actor_queue_returns_busy_without_accepting_or_recording_a_write() {
    let fixture = Fixture::new();
    let notices = ready(&fixture);
    let connection = fixture.connection();
    let client = actor(&fixture);
    let (barrier, entered, release) = client.test_barrier().unwrap();
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    let queued = (0..128)
        .map(|_| client.list_catalog().unwrap())
        .collect::<Vec<_>>();
    let response = fixture.call(
        &connection,
        101,
        Command::CreateCollection {
            name: "never accepted".into(),
        },
    );
    release.send(()).unwrap();
    barrier
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    for reply in queued {
        assert!(
            reply
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap()
                .definitions
                .is_empty()
        );
    }
    assert_eq!(response.result.unwrap_err().code, ErrorCode::Busy);
    assert!(
        fixture
            .handlers
            .changes
            .records()
            .lock()
            .unwrap()
            .entries()
            .is_empty()
    );
    assert!(notices.try_recv().is_err());
}

#[test]
fn pre_enqueue_cancel_or_deadline_leaves_actor_unchanged_and_keeps_worker_permit() {
    for deadline in [false, true] {
        let mut fixture = Fixture::new();
        let seconds = clock(&mut fixture);
        let (entered_send, entered) = crossbeam_channel::bounded(1);
        let (release, gate) = crossbeam_channel::bounded(1);
        Arc::get_mut(&mut fixture.handlers).unwrap().write_hooks =
            Some(Arc::new(collection_write::WriteHooks {
                before_enqueue: Some(Arc::new(move || {
                    entered_send.send(()).unwrap();
                    gate.recv_timeout(Duration::from_secs(10)).unwrap();
                })),
                after_enqueue: None,
            }));
        let notices = ready(&fixture);
        let connection = fixture.connection();
        let (worker, replies) = start(
            &connection,
            Command::CreateCollection {
                name: "cancelled".into(),
            },
        );
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        if deadline {
            seconds.store(21, Ordering::Release);
        } else {
            fixture.call(&connection, 202, Command::Cancel { request_id: 201 });
        }
        let reply = replies
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .wire();
        assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 1);
        release.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(
            reply.result.unwrap_err().code,
            if deadline {
                ErrorCode::Timeout
            } else {
                ErrorCode::Cancelled
            }
        );
        assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
        assert!(
            actor(&fixture)
                .list_catalog()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap()
                .definitions
                .is_empty()
        );
        assert!(
            fixture
                .handlers
                .changes
                .records()
                .lock()
                .unwrap()
                .entries()
                .is_empty()
        );
        assert!(notices.try_recv().is_err());
        assert!(replies.try_recv().is_err());
    }
}

#[test]
fn accepted_write_keeps_result_and_attribution_after_deadline_cancel_disconnect_and_permission_drop()
 {
    let mut fixture = Fixture::new();
    let seconds = clock(&mut fixture);
    let (entered_send, entered) = crossbeam_channel::bounded(1);
    let (release, gate) = crossbeam_channel::bounded(1);
    Arc::get_mut(&mut fixture.handlers).unwrap().write_hooks =
        Some(Arc::new(collection_write::WriteHooks {
            before_enqueue: None,
            after_enqueue: Some(Arc::new(move || {
                entered_send.send(()).unwrap();
                gate.recv_timeout(Duration::from_secs(10)).unwrap();
            })),
        }));
    let notices = ready(&fixture);
    let connection = fixture.connection();
    let (worker, replies) = start(
        &connection,
        Command::CreateCollection {
            name: "accepted".into(),
        },
    );
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    seconds.store(21, Ordering::Release);
    assert!(
        fixture
            .call(&connection, 202, Command::Cancel { request_id: 201 })
            .result
            .is_ok()
    );
    connection.disconnect();
    fixture.call(
        &connection,
        203,
        Command::Hello {
            version: AGENT_PROTOCOL_VERSION,
            client_name: "later client".into(),
            client_version: "2".into(),
            request_kind: RequestKind::Tool,
        },
    );
    fixture.set_level(AgentLevel::ReadOnly);
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 1);
    assert!(matches!(
        replies.try_recv(),
        Err(crossbeam_channel::TryRecvError::Empty)
    ));
    release.send(()).unwrap();
    let reply = replies.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    fixture.set_level(AgentLevel::Disabled);
    let id = match reply.wire().result.unwrap() {
        Payload::CollectionWrite(CollectionWriteResult::Created { id }) => id,
        result => panic!("expected accepted actor result, got {result:?}"),
    };
    assert_eq!(snapshot(&fixture, &id).definition.name, "accepted");
    assert_eq!(fixture.handlers.active.load(Ordering::Acquire), 0);
    let shared = fixture.handlers.changes.records();
    let records = shared.lock().unwrap();
    assert_eq!(records.entries().len(), 1);
    assert_eq!(records.entries()[0].client_name, "test");
    assert_eq!(records.entries()[0].counts.changed, 1);
    drop(records);
    assert_eq!(notices.try_iter().count(), 1);
    assert!(replies.try_recv().is_err());
}

// Real filesystem paths are accepted by the IPC contract only on Windows.
#[cfg(windows)]
mod filesystem {
    use super::*;

    #[test]
    fn add_preserves_input_positions_and_reports_duplicates_and_every_skip_reason() {
        let fixture = Fixture::new();
        let notices = ready(&fixture);
        let connection = fixture.connection();
        let id = create(&fixture, &connection, "items");
        let first = fixture._dir.path().join("one.png");
        let second = fixture._dir.path().join("two.jpg");
        let other = fixture._dir.path().join("unsupported.xyzabc");
        for path in [&first, &second, &other] {
            std::fs::write(path, b"fixture").unwrap();
        }
        let missing = fixture._dir.path().join("missing.png");
        let mut zip_entry = item(&first);
        zip_entry.subresource = Some("entry.png".into());
        let mut pdf_page = item(&second);
        pdf_page.subresource = Some("page:1".into());
        let response = fixture.call(
            &connection,
            102,
            Command::AddToCollection {
                id: id.clone(),
                items: vec![
                    item(&second),
                    item(&first),
                    item(&second),
                    item(&missing),
                    ItemRef {
                        path: "relative.png".into(),
                        subresource: None,
                    },
                    zip_entry,
                    pdf_page,
                    item(&other),
                ],
            },
        );
        let results = outcomes(response, &id);
        assert!(results.contains(&CollectionItemOutcome::Added {
            indices: vec![0, 1]
        }));
        assert!(results.contains(&CollectionItemOutcome::Duplicate { indices: vec![2] }));
        for (indices, reason) in [
            (vec![3], CollectionSkipReason::NotFound),
            (vec![4], CollectionSkipReason::InvalidPath),
            (vec![5, 6], CollectionSkipReason::UnsupportedSubresource),
            (vec![7], CollectionSkipReason::UnsupportedType),
        ] {
            assert!(results.contains(&CollectionItemOutcome::Skipped { indices, reason }));
        }
        let after = snapshot(&fixture, &id);
        assert_eq!(after.entries.len(), 2);
        assert_eq!(after.entries[0].source_path, second);
        assert_eq!(after.entries[1].source_path, first);
        let duplicate = outcomes(
            fixture.call(
                &connection,
                103,
                Command::AddToCollection {
                    id: id.clone(),
                    items: vec![item(&first), item(&second)],
                },
            ),
            &id,
        );
        assert_eq!(
            duplicate,
            vec![CollectionItemOutcome::Duplicate {
                indices: vec![0, 1]
            }]
        );
        assert_eq!(snapshot(&fixture, &id), after);
        let shared = fixture.handlers.changes.records();
        let records = shared.lock().unwrap();
        let add = &records.entries()[1];
        assert_eq!(add.counts.changed, 2);
        assert_eq!(add.counts.unchanged, 1);
        assert_eq!(add.counts.skipped, 5);
        assert_eq!(add.rows.len(), 8);
        assert_eq!(add.rows[0].item, second.to_string_lossy());
        assert_eq!(add.rows[1].item, first.to_string_lossy());
        assert_ne!(add.rows[0].before, add.rows[0].after);
        drop(records);
        assert_eq!(notices.try_iter().count(), 3);
    }

    #[test]
    fn remove_matches_source_keys_after_file_disappears_and_without_type_validation() {
        use crate::collection_store::{CollectionRegistration, CollectionResolvedKind};
        let fixture = Fixture::new();
        let notices = ready(&fixture);
        let connection = fixture.connection();
        let id = create(&fixture, &connection, "remove");
        let missing = fixture._dir.path().join("gone.png");
        let unsupported = fixture._dir.path().join("old.xyzabc");
        std::fs::write(&missing, b"fixture").unwrap();
        std::fs::write(&unsupported, b"fixture").unwrap();
        let before = snapshot(&fixture, &id);
        actor(&fixture)
            .add_batch(
                before.collection_id(),
                before.revision(),
                vec![
                    CollectionRegistration::from_trusted_path(
                        &missing,
                        CollectionResolvedKind::Image,
                    )
                    .unwrap(),
                    CollectionRegistration::from_trusted_path(
                        &unsupported,
                        CollectionResolvedKind::Unresolved,
                    )
                    .unwrap(),
                ],
            )
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        std::fs::remove_file(&missing).unwrap();
        let alias = ItemRef {
            path: missing.to_string_lossy().replace('\\', "/").to_uppercase(),
            subresource: None,
        };
        let absent = fixture._dir.path().join("never-added.anything");
        let removed = outcomes(
            fixture.call(
                &connection,
                102,
                Command::RemoveFromCollection {
                    id: id.clone(),
                    items: vec![alias, item(&missing), item(&unsupported), item(&absent)],
                },
            ),
            &id,
        );
        assert!(removed.contains(&CollectionItemOutcome::Removed {
            indices: vec![0, 2]
        }));
        assert!(removed.contains(&CollectionItemOutcome::NotPresent {
            indices: vec![1, 3]
        }));
        assert!(snapshot(&fixture, &id).entries.is_empty());
        let shared = fixture.handlers.changes.records();
        let records = shared.lock().unwrap();
        let record = &records.entries()[0];
        assert_eq!(record.counts.changed, 2);
        assert_eq!(record.counts.unchanged, 2);
        assert_eq!(record.rows.len(), 4);
        assert_ne!(record.rows[0].before, record.rows[0].after);
        drop(records);
        assert_eq!(notices.try_iter().count(), 2);
    }

    #[test]
    fn five_hundred_items_are_accepted_and_five_hundred_one_are_rejected_without_changes() {
        let fixture = Fixture::new();
        let notices = ready(&fixture);
        let connection = fixture.connection();
        let id = create(&fixture, &connection, "limit");
        let path = fixture._dir.path().join("one.png");
        std::fs::write(&path, b"fixture").unwrap();
        let results = outcomes(
            fixture.call(
                &connection,
                102,
                Command::AddToCollection {
                    id: id.clone(),
                    items: vec![item(&path); 500],
                },
            ),
            &id,
        );
        assert!(results.contains(&CollectionItemOutcome::Added { indices: vec![0] }));
        assert!(results.contains(&CollectionItemOutcome::Duplicate {
            indices: (1..500).collect()
        }));
        let before = snapshot(&fixture, &id);
        for command in [
            Command::AddToCollection {
                id: id.clone(),
                items: vec![item(&path); 501],
            },
            Command::RemoveFromCollection {
                id: id.clone(),
                items: vec![item(&path); 501],
            },
        ] {
            assert_eq!(
                fixture
                    .call(&connection, 103, command)
                    .result
                    .unwrap_err()
                    .code,
                ErrorCode::BadRequest
            );
        }
        assert_eq!(snapshot(&fixture, &id), before);
        let shared = fixture.handlers.changes.records();
        let records = shared.lock().unwrap();
        assert_eq!(records.entries().len(), 2);
        assert_eq!(records.entries()[0].rows.len(), 500);
        assert_eq!(notices.try_iter().count(), 2);
    }

    #[test]
    fn capacity_rejection_and_repeated_inputs_keep_the_first_occurrence_result() {
        use crate::collection_store::{
            CollectionRegistration, CollectionResolvedKind, MAX_COLLECTION_ENTRIES,
        };
        let fixture = Fixture::new();
        let notices = ready(&fixture);
        let connection = fixture.connection();
        let id = create(&fixture, &connection, "capacity");
        let before = snapshot(&fixture, &id);
        let registrations = (0..MAX_COLLECTION_ENTRIES - 1)
            .map(|index| {
                CollectionRegistration::from_trusted_path(
                    fixture._dir.path().join(format!("seed{index}.png")),
                    CollectionResolvedKind::Unresolved,
                )
                .unwrap()
            })
            .collect();
        actor(&fixture)
            .add_batch(before.collection_id(), before.revision(), registrations)
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let fits = fixture._dir.path().join("fits.png");
        let rejected = fixture._dir.path().join("rejected.png");
        std::fs::write(&fits, b"fixture").unwrap();
        std::fs::write(&rejected, b"fixture").unwrap();
        let result = outcomes(
            fixture.call(
                &connection,
                102,
                Command::AddToCollection {
                    id: id.clone(),
                    items: vec![item(&fits), item(&rejected), item(&rejected)],
                },
            ),
            &id,
        );
        assert!(result.contains(&CollectionItemOutcome::Added { indices: vec![0] }));
        assert!(result.contains(&CollectionItemOutcome::CapacityRejected { indices: vec![1] }));
        assert!(result.contains(&CollectionItemOutcome::Duplicate { indices: vec![2] }));
        let after = snapshot(&fixture, &id);
        assert_eq!(after.entries.len(), MAX_COLLECTION_ENTRIES);
        assert!(after.entries.iter().any(|entry| entry.source_path == fits));
        assert!(
            !after
                .entries
                .iter()
                .any(|entry| entry.source_path == rejected)
        );
        assert_eq!(notices.try_iter().count(), 2);
    }
}
