use super::*;
use crate::transcript_db::{RemovedCounts, SaveRequest, TranscriptResult};

fn fake_cache() -> (
    TranscriptDisplayCache,
    Receiver<ReadRequest>,
    Sender<ReadResult>,
) {
    let (requests, request_rx) = mpsc::channel();
    let (result_tx, results) = mpsc::channel();
    (
        TranscriptDisplayCache {
            worker: Some(Worker { requests, results }),
            ..Default::default()
        },
        request_rx,
        result_tx,
    )
}

fn value(text: &str) -> TranscriptDisplayValue {
    let signature = FileSignature {
        size: 4,
        mtime_ms: 100,
    };
    TranscriptDisplayValue {
        row: Some(TranscriptDisplayRow {
            signature,
            audio_stream: Some(1),
            model_id: "model@revision".into(),
            config_version: 1,
            status: TranscriptStatus::Ok,
            created_at: 20,
            segments: Arc::new(vec![(0, 3000, text.into())]),
        }),
        current_signature: Some(signature),
        read_failed: false,
    }
}

fn result(request: ReadRequest, value: TranscriptDisplayValue) -> ReadResult {
    ReadResult {
        key: request.key,
        epoch: request.epoch,
        generation: request.generation,
        value,
    }
}

#[test]
fn transcript_cache_deduplicates_pending_and_retains_previous_value_during_refresh() {
    let (mut cache, requests, results) = fake_cache();
    let ctx = egui::Context::default();
    let path = Path::new("a.mp4");
    assert!(cache.request(&ctx, "a", path, false).unwrap());
    assert!(cache.contains("a"));
    assert!(!cache.needs_request("a", false));
    assert!(cache.get("a").is_none());
    assert!(!cache.request(&ctx, "a", path, false).unwrap());
    let initial = requests.try_recv().unwrap();
    assert!(requests.try_recv().is_err());
    let old = value("古い本文");
    let old_segments = Arc::clone(&old.row.as_ref().unwrap().segments);
    results.send(result(initial, old)).unwrap();
    assert!(cache.poll());
    assert!(Arc::ptr_eq(
        &cache.get("a").unwrap().row.as_ref().unwrap().segments,
        &old_segments
    ));
    assert!(!cache.request(&ctx, "a", path, false).unwrap());
    assert!(cache.request(&ctx, "a", path, true).unwrap());
    assert!(Arc::ptr_eq(
        &cache.get("a").unwrap().row.as_ref().unwrap().segments,
        &old_segments
    ));
    let superseded_refresh = requests.try_recv().unwrap();
    assert!(cache.request(&ctx, "a", path, true).unwrap());
    assert!(Arc::ptr_eq(
        &cache.get("a").unwrap().row.as_ref().unwrap().segments,
        &old_segments
    ));
    let refresh = requests.try_recv().unwrap();
    results
        .send(result(superseded_refresh, value("古い世代の再読取り")))
        .unwrap();
    assert!(!cache.poll());
    assert!(Arc::ptr_eq(
        &cache.get("a").unwrap().row.as_ref().unwrap().segments,
        &old_segments
    ));
    let fresh = value("新しい本文");
    let fresh_segments = Arc::clone(&fresh.row.as_ref().unwrap().segments);
    results.send(result(refresh, fresh)).unwrap();
    assert!(cache.poll());
    assert!(Arc::ptr_eq(
        &cache.get("a").unwrap().row.as_ref().unwrap().segments,
        &fresh_segments
    ));
    assert!(!Arc::ptr_eq(
        &cache.get("a").unwrap().row.as_ref().unwrap().segments,
        &old_segments
    ));
    assert!(!cache.poll());
}

#[test]
fn transcript_cache_clear_rejects_old_epoch_and_reloads_same_key_without_old_arc() {
    let (mut cache, requests, results) = fake_cache();
    let ctx = egui::Context::default();
    cache.seed("a", value("表示済み"));
    let old_arc = Arc::clone(&cache.get("a").unwrap().row.as_ref().unwrap().segments);
    cache.request(&ctx, "a", Path::new("a"), true).unwrap();
    let old = requests.try_recv().unwrap();
    let epoch = cache.epoch_for_test();
    cache.clear();
    assert_ne!(cache.epoch_for_test(), epoch);
    assert!(!cache.contains("a"));
    assert!(cache.get("a").is_none());
    assert!(cache.request(&ctx, "a", Path::new("a"), false).unwrap());
    assert!(cache.get("a").is_none());
    let fresh = requests.try_recv().unwrap();
    results.send(result(old, value("消去前の結果"))).unwrap();
    assert!(!cache.poll());
    assert!(cache.get("a").is_none());
    results.send(result(fresh, value("消去後の結果"))).unwrap();
    assert!(cache.poll());
    assert!(!Arc::ptr_eq(
        &cache.get("a").unwrap().row.as_ref().unwrap().segments,
        &old_arc
    ));
}

#[test]
fn transcript_cache_invalidate_and_refresh_use_unique_generations_for_same_key() {
    let (mut cache, requests, results) = fake_cache();
    let ctx = egui::Context::default();
    cache.seed("a", value("無効化前の表示"));
    cache.request(&ctx, "a", Path::new("a"), true).unwrap();
    assert!(cache.get("a").is_some());
    let invalidated = requests.try_recv().unwrap();
    cache.invalidate("a");
    cache.request(&ctx, "a", Path::new("a"), false).unwrap();
    assert!(cache.get("a").is_none());
    let superseded = requests.try_recv().unwrap();
    cache.request(&ctx, "a", Path::new("a"), true).unwrap();
    let current = requests.try_recv().unwrap();
    assert_ne!(invalidated.generation, superseded.generation);
    assert_ne!(superseded.generation, current.generation);
    results
        .send(result(invalidated, value("無効化済み")))
        .unwrap();
    results.send(result(superseded, value("再入場前"))).unwrap();
    assert!(!cache.poll());
    assert!(cache.get("a").is_none());
    results.send(result(current, value("最新"))).unwrap();
    assert!(cache.poll());
    assert_eq!(
        cache.get("a").unwrap().row.as_ref().unwrap().segments[0].2,
        "最新"
    );
}

#[test]
fn transcript_cache_results_for_other_keys_keep_their_own_values_and_arc_identity() {
    let (mut cache, requests, results) = fake_cache();
    let ctx = egui::Context::default();
    cache.request(&ctx, "a", Path::new("a"), false).unwrap();
    cache.request(&ctx, "b", Path::new("b"), false).unwrap();
    let a_request = requests.try_recv().unwrap();
    let b_request = requests.try_recv().unwrap();
    let a = value("Aの本文");
    let a_arc = Arc::clone(&a.row.as_ref().unwrap().segments);
    results.send(result(b_request, value("Bの本文"))).unwrap();
    assert!(cache.poll());
    assert!(cache.get("a").is_none());
    assert_eq!(
        cache.get("b").unwrap().row.as_ref().unwrap().segments[0].2,
        "Bの本文"
    );
    results.send(result(a_request, a)).unwrap();
    assert!(cache.poll());
    let snapshot = cache.get("a").unwrap().clone();
    assert!(Arc::ptr_eq(
        &snapshot.row.as_ref().unwrap().segments,
        &a_arc
    ));
    cache.invalidate("b");
    assert!(Arc::ptr_eq(
        &cache.get("a").unwrap().row.as_ref().unwrap().segments,
        &a_arc
    ));
}

#[test]
fn transcript_cache_start_and_send_errors_are_terminal_until_explicit_refresh() {
    let ctx = egui::Context::default();
    let mut cache = TranscriptDisplayCache::default();
    assert!(
        cache
            .request_with_start(&ctx, "a", Path::new("a"), false, || Err(
                std::io::Error::other("synthetic startup failure")
            ))
            .is_err()
    );
    assert!(cache.get("a").unwrap().read_failed);
    assert!(!cache.needs_request("a", false));
    assert!(
        !cache
            .request_with_start(&ctx, "a", Path::new("a"), false, || panic!(
                "同じ失敗を毎フレーム再試行してはいけない"
            ))
            .unwrap()
    );

    let (mut cache, requests, _results) = fake_cache();
    drop(requests);
    assert!(cache.request(&ctx, "a", Path::new("a"), false).is_err());
    assert!(cache.get("a").unwrap().read_failed);
    assert!(!cache.request(&ctx, "a", Path::new("a"), false).unwrap());
}

#[test]
fn transcript_cache_limits_loaded_and_pending_entries_to_32_and_retires_old_results() {
    let (mut cache, requests, results) = fake_cache();
    let ctx = egui::Context::default();
    for i in 0..16 {
        cache.seed(&format!("loaded-{i}"), value("本文"));
    }
    for i in 0..16 {
        cache
            .request(&ctx, &format!("pending-{i}"), Path::new("p"), false)
            .unwrap();
    }
    assert_eq!(cache.entries.len(), MAX_ENTRIES);
    let old = requests.try_recv().unwrap();
    let epoch = cache.epoch_for_test();
    cache.request(&ctx, "33rd", Path::new("p"), false).unwrap();
    assert_ne!(cache.epoch_for_test(), epoch);
    assert_eq!(cache.entries.len(), 1);
    assert!(cache.contains("33rd"));
    assert!(!cache.contains("loaded-0"));
    results
        .send(result(old, value("上限消去前の結果")))
        .unwrap();
    assert!(!cache.poll());
    assert_eq!(cache.entries.len(), 1);
}

fn real_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("voice.mp4");
    let db_path = dir.path().join("transcript.db");
    std::fs::write(&path, b"audio").unwrap();
    let signature = FileSignature::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let db = TranscriptDb::open_at(&db_path).unwrap();
    db.save(&SaveRequest {
        item_path: path.clone(),
        signature,
        model_id: "fixture@revision".into(),
        config_version: 1,
        lang: "ja".into(),
        result: TranscriptResult::Ok {
            audio_stream: 1,
            duration_ms: 3000,
            segments: vec![(0, 3000, "保存した本文".into())],
            removed_counts: RemovedCounts::default(),
        },
    })
    .unwrap();
    let key = TranscriptDb::normalize_path(&path);
    (dir, db_path, path, key)
}

#[test]
fn transcript_display_row_consumes_db_vector_without_copying_segment_allocation() {
    let (_dir, db_path, path, key) = real_fixture();
    let db = TranscriptDb::open_read_only(&db_path).unwrap().unwrap();
    let row = db.get(&key).unwrap().unwrap();
    let original = row.segments.as_ptr();
    let display = TranscriptDisplayRow::from(row);
    assert_eq!(display.segments.as_ptr(), original);
    let value = read_value(&db_path, &key, &path);
    assert!(!value.read_failed);
    assert!(!value.is_stale());
    let actual = value.row.unwrap();
    assert_eq!(actual, display);
    assert_eq!(actual.audio_stream, Some(1));
}

#[test]
fn transcript_cache_real_worker_reads_without_ui_io_and_adopts_one_arc() {
    let (_dir, db_path, path, key) = real_fixture();
    let mut cache = TranscriptDisplayCache::default();
    cache
        .request_with_start(&egui::Context::default(), &key, &path, false, || {
            Worker::start(Some(db_path))
        })
        .unwrap();
    let result = cache
        .worker
        .as_ref()
        .unwrap()
        .results
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    let arc = Arc::clone(&result.value.row.as_ref().unwrap().segments);
    assert!(cache.adopt(result));
    assert!(Arc::ptr_eq(
        &cache.get(&key).unwrap().row.as_ref().unwrap().segments,
        &arc
    ));
}

#[test]
fn transcript_cache_reads_do_not_create_missing_store_or_publish_uninitialized_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("voice.mp4");
    std::fs::write(&path, b"audio").unwrap();
    let db_path = dir.path().join("missing/transcript.db");
    let value = read_value(&db_path, "voice", &path);
    assert!(!value.read_failed);
    assert!(value.row.is_none());
    assert!(value.current_signature.is_some());
    assert!(!db_path.parent().unwrap().exists());
    let unpublished = dir.path().join("unpublished.db");
    rusqlite::Connection::open(&unpublished).unwrap();
    assert!(!read_value(&unpublished, "voice", &path).read_failed);
    let conn = rusqlite::Connection::open(&unpublished).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| row
            .get::<_, usize>(0))
            .unwrap(),
        0
    );
}

#[test]
fn transcript_cache_read_error_is_terminal_and_signature_comparison_leaves_missing_file_to_lifecycle()
 {
    let (_dir, db_path, path, key) = real_fixture();
    std::fs::write(&path, b"externally overwritten with longer audio").unwrap();
    assert!(read_value(&db_path, &key, &path).is_stale());
    std::fs::remove_file(&path).unwrap();
    let missing = read_value(&db_path, &key, &path);
    assert!(missing.row.is_some());
    assert!(missing.current_signature.is_none());
    assert!(!missing.is_stale());
    assert!(!missing.read_failed);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("DROP TABLE transcripts").unwrap();
    let failed = read_value(&db_path, &key, &path);
    assert!(failed.read_failed);
    assert!(failed.row.is_none());
    let (mut cache, requests, results) = fake_cache();
    cache
        .request(&egui::Context::default(), &key, &path, false)
        .unwrap();
    results
        .send(result(requests.try_recv().unwrap(), failed))
        .unwrap();
    assert!(cache.poll());
    assert!(cache.get(&key).unwrap().read_failed);
    assert!(!cache.needs_request(&key, false));
    assert!(
        !cache
            .request(&egui::Context::default(), &key, &path, false)
            .unwrap()
    );
    assert!(requests.try_recv().is_err());
}
