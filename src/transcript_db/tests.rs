use super::*;

fn fixture() -> (tempfile::TempDir, TranscriptDb, SaveRequest) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("voice.mp4");
    std::fs::write(&path, b"audio").unwrap();
    let request = SaveRequest {
        signature: FileSignature::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        item_path: path,
        model_id: "whisper@abc".into(),
        config_version: 1,
        lang: "ja".into(),
        result: TranscriptResult::Ok {
            segments: vec![
                (0, 2000, "最初の言葉".into()),
                (2000, 4000, "次の言葉".into()),
            ],
            removed_counts: RemovedCounts {
                stock: 2,
                repeat: 3,
            },
            duration_ms: 4000,
            audio_stream: 1,
        },
    };
    let db = TranscriptDb::open_at(dir.path().join("transcript.db")).unwrap();
    (dir, db, request)
}
fn read(db: &TranscriptDb, request: &SaveRequest) -> TranscriptRow {
    db.get(request.item_path.to_str().unwrap())
        .unwrap()
        .unwrap()
}
fn seq(db: &TranscriptDb) -> i64 {
    db.conn
        .query_row("SELECT n FROM transcript_seq", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn transcript_roundtrip_preserves_tuple_json_and_counts() {
    let (dir, db, req) = fixture();
    assert_eq!(db.save(&req).unwrap(), SaveOutcome::Written);
    let row = read(&db, &req);
    assert_eq!(row.signature, req.signature);
    assert_eq!(row.status, TranscriptStatus::Ok);
    assert_eq!(row.audio_stream, Some(1));
    assert_eq!(
        row.removed_counts,
        RemovedCounts {
            stock: 2,
            repeat: 3
        }
    );
    let raw: String = db
        .conn
        .query_row("SELECT segments FROM transcripts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&raw).unwrap(),
        serde_json::json!([[0, 2000, "最初の言葉"], [2000, 4000, "次の言葉"]])
    );
    assert_eq!(row.rev, 1);
    assert!(!row.indexed);
    let readonly = TranscriptDb::open_read_only(dir.path().join("transcript.db"))
        .unwrap()
        .unwrap();
    assert_eq!(read(&readonly, &req), row);
}

#[test]
fn transcript_failed_never_replaces_ok_and_only_ok_is_skipped() {
    let (_dir, db, mut req) = fixture();
    let success = req.result.clone();
    req.result = TranscriptResult::Failed;
    assert_eq!(db.save(&req).unwrap(), SaveOutcome::Written);
    assert!(!db.has_ok(req.item_path.to_str().unwrap()).unwrap());
    let failed = read(&db, &req);
    assert_eq!(failed.audio_stream, None);
    assert!(failed.segments.is_empty());
    req.result = success;
    assert_eq!(db.save(&req).unwrap(), SaveOutcome::Written);
    let ok = read(&db, &req);
    assert!(db.has_ok(req.item_path.to_str().unwrap()).unwrap());
    req.result = TranscriptResult::Failed;
    assert_eq!(db.save(&req).unwrap(), SaveOutcome::KeptExistingOk);
    assert_eq!(read(&db, &req), ok);
    // 署名が古くても成功行を飛ばす規則は変えない。
    std::fs::write(&req.item_path, b"changed audio").unwrap();
    assert!(db.has_ok(req.item_path.to_str().unwrap()).unwrap());
}

#[test]
fn transcript_signature_change_or_missing_input_is_discarded_without_mutation() {
    let (_dir, db, mut req) = fixture();
    db.save(&req).unwrap();
    let original = read(&db, &req);
    req.signature.size += 1;
    assert_eq!(db.save(&req).unwrap(), SaveOutcome::Discarded);
    req.signature.size -= 1;
    req.signature.mtime_ms += 1;
    assert_eq!(db.save(&req).unwrap(), SaveOutcome::Discarded);
    std::fs::remove_file(&req.item_path).unwrap();
    assert_eq!(db.save(&req).unwrap(), SaveOutcome::Discarded);
    assert_eq!(read(&db, &req), original);
    assert_eq!(seq(&db), 1);
}

#[test]
fn transcript_triggers_cover_copy_update_rename_delete_and_ack_is_inert() {
    let (_dir, db, req) = fixture();
    db.save(&req).unwrap();
    db.conn
        .execute_batch("UPDATE transcripts SET indexed=1;")
        .unwrap();
    assert_eq!(seq(&db), 1);
    db.conn.execute_batch("INSERT INTO transcripts SELECT 'c:/copy.mp4',mtime,size,audio_stream,model_id,config_version,lang,duration_ms,segments,removed_counts,status,created_at,rev,indexed FROM transcripts;").unwrap();
    let copied = db.get("c:/copy.mp4").unwrap().unwrap();
    assert_eq!(copied.rev, 2);
    assert!(!copied.indexed);
    db.conn
        .execute_batch(
            "UPDATE transcripts SET item_key='c:/moved.mp4' WHERE item_key='c:/copy.mp4';",
        )
        .unwrap();
    assert_eq!(db.get("c:/moved.mp4").unwrap().unwrap().rev, 3);
    assert_eq!(db.delete("c:/moved.mp4").unwrap(), 1);
    assert_eq!(seq(&db), 4);
    let removed: Vec<(String, i64)> = db
        .conn
        .prepare("SELECT item_key,rev FROM transcript_removed ORDER BY rev")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        removed,
        vec![("c:/copy.mp4".into(), 3), ("c:/moved.mp4".into(), 4)]
    );
    assert!(read(&db, &req).indexed);
}

#[test]
fn transcript_bulk_delete_counts_status_and_rolls_back_as_one_transaction() {
    let (dir, db, mut req) = fixture();
    db.save(&req).unwrap();
    let first_key = TranscriptDb::normalize_path(&req.item_path);
    req.item_path = dir.path().join("second.mp4");
    std::fs::write(&req.item_path, b"audio").unwrap();
    req.signature =
        FileSignature::from_metadata(&std::fs::metadata(&req.item_path).unwrap()).unwrap();
    req.result = TranscriptResult::Failed;
    db.save(&req).unwrap();
    let second_key = TranscriptDb::normalize_path(&req.item_path);
    let keys = vec![first_key.clone(), second_key.clone()];
    assert_eq!(
        db.count_existing(&keys).unwrap(),
        TranscriptDeletionCandidates {
            keys: keys.clone(),
            ok: 1,
            failed: 1
        }
    );
    db.conn.execute_batch("CREATE TRIGGER deny_delete BEFORE DELETE ON transcripts WHEN OLD.status='failed' BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
    assert!(db.delete_keys(&keys).is_err());
    assert!(db.get(&first_key).unwrap().is_some());
    assert_eq!(seq(&db), 2);
    db.conn.execute_batch("DROP TRIGGER deny_delete").unwrap();
    assert_eq!(db.delete_keys(&keys).unwrap(), 2);
    assert_eq!(
        db.count_existing(&keys).unwrap(),
        TranscriptDeletionCandidates::default()
    );
    assert_eq!(seq(&db), 4);
}

#[test]
fn transcript_cancel_before_save_does_not_write_but_after_commit_keeps_written() {
    let (_dir, db, req) = fixture();
    let cancel = Arc::new(AtomicBool::new(true));
    assert_eq!(db.save_unless_cancelled(&req, &cancel).unwrap(), None);
    assert_eq!(seq(&db), 0);
    cancel.store(false, Ordering::Release);
    let outcome = db.save_unless_cancelled(&req, &cancel).unwrap();
    cancel.store(true, Ordering::Release);
    assert_eq!(outcome, Some(SaveOutcome::Written));
    assert!(db.has_ok(req.item_path.to_str().unwrap()).unwrap());
}

#[test]
fn transcript_reader_does_not_create_or_initialize_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing/transcript.db");
    assert!(TranscriptDb::open_read_only(&path).unwrap().is_none());
    assert!(!path.parent().unwrap().exists());
    let unpublished = dir.path().join("unpublished.db");
    Connection::open(&unpublished).unwrap();
    assert!(
        TranscriptDb::open_read_only(&unpublished)
            .unwrap()
            .is_none()
    );
}

#[test]
fn transcript_validation_accepts_transport_zero_length_overlap_and_rounded_end() {
    assert!(
        validate_transcript(
            &[
                (0, 0, "語".into()),
                (0, 40, "別の語".into()),
                (20, 50, "重なる語".into())
            ],
            49
        )
        .is_ok()
    );
    assert!(validate_transcript(&[(40, 30, "語".into())], 50).is_err());
    assert!(validate_transcript(&[(30, 40, "語".into()), (20, 50, "語".into())], 50).is_err());
    assert!(validate_transcript(&[(0, 0, " ".into())], 0).is_err());
    assert!(validate_transcript(&[], 0).is_ok());
}

#[test]
fn transcript_save_rechecks_signature_after_rename_writer_commits() {
    thread_local! {
        static FENCE: std::cell::RefCell<Option<(std::sync::mpsc::Sender<()>,std::sync::mpsc::Receiver<()>)>> = const { std::cell::RefCell::new(None) };
    }
    fn waiting(_: i32) -> bool {
        FENCE.with(|slot| {
            if let Some((entered, committed)) = slot.borrow_mut().take() {
                entered.send(()).unwrap();
                committed.recv_timeout(Duration::from_secs(5)).unwrap();
                true
            } else {
                false
            }
        })
    }
    let (dir, db, req) = fixture();
    db.save(&req).unwrap();
    let saving = TranscriptDb::open_existing_for_write(dir.path().join("transcript.db")).unwrap();
    let lock = Transaction::new_unchecked(&db.conn, TransactionBehavior::Immediate).unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (committed_tx, committed_rx) = std::sync::mpsc::channel();
    let original = req.item_path.clone();
    let worker = std::thread::spawn(move || {
        FENCE.with(|slot| *slot.borrow_mut() = Some((entered_tx, committed_rx)));
        saving.conn.busy_handler(Some(waiting)).unwrap();
        saving.save(&req)
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let renamed = dir.path().join("renamed.mp4");
    std::fs::rename(&original, &renamed).unwrap();
    let renamed_key = TranscriptDb::normalize_path(&renamed);
    lock.execute("UPDATE transcripts SET item_key=?1", [&renamed_key])
        .unwrap();
    lock.commit().unwrap();
    committed_tx.send(()).unwrap();
    assert_eq!(worker.join().unwrap().unwrap(), SaveOutcome::Discarded);
    assert!(db.get(original.to_str().unwrap()).unwrap().is_none());
    assert!(db.get(&renamed_key).unwrap().is_some());
    assert_eq!(seq(&db), 2);
}
