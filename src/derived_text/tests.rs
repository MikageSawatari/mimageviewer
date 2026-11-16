use super::test_support::{candidate, save, stored};
use super::*;
use std::sync::{Arc, atomic::AtomicBool};

#[test]
fn transcript_ingest_keeps_ok_and_stale_but_excludes_failed_and_missing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::transcript_db::TranscriptDb::open_at(dir.path().join("transcript.db")).unwrap();
    let meta = crate::fts_meta::FtsMetaDb::open_at(&dir.path().join("meta.db")).unwrap();
    let fts = Arc::new(crate::fts_index::FtsIndex::open_at(&dir.path().join("index")).unwrap());
    let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
        fts.writer().unwrap(),
        fts.clone(),
    );
    let mut paths = Vec::new();
    for (name, text) in [
        ("ok.mp4", Some("Mango orchard")),
        ("stale.mp3", Some("Stale orchard")),
        ("failed.mp3", None),
    ] {
        let path = dir.path().join(name);
        save(&db, &path, text);
        paths.push(path);
    }
    std::fs::write(
        &paths[1],
        b"changed signature, saved text remains searchable",
    )
    .unwrap();
    let missing = dir.path().join("missing.mp3");
    std::fs::write(&missing, b"no row").unwrap();
    paths.push(missing);
    let session = crate::ingest_worker::IngestSession::new_with_derived_text_paths(
        uuid::Uuid::new_v4(),
        dir.path().to_owned(),
        &meta,
        &fts,
        DerivedTextPaths::at(dir.path()),
    );
    let stats = session
        .apply(
            paths.iter().map(|path| candidate(path)).collect(),
            vec![],
            &writer,
            &crate::io_semaphore::GlobalIoSemaphore::new(1),
            crate::io_semaphore::IoPriority::Low,
            &AtomicBool::new(false),
            None,
        )
        .unwrap();
    assert_eq!(stats.ingested_ok, 4);
    assert_eq!(stats.derived_text_read_errors, 0);
    for (index, expected) in ["mango orchard", "stale orchard", "", ""]
        .iter()
        .enumerate()
    {
        let key = crate::transcript_db::TranscriptDb::normalize_path(&paths[index]);
        assert_eq!(&stored(&fts, &key).unwrap().transcript, expected);
    }
    let query = crate::fts_index::build_bigram_and_query(
        fts.fields(),
        &["orchard"],
        &crate::fts_index::QueryFilters {
            target: crate::fts_index::SearchTarget::Only(vec![
                crate::fts_index::SourceKind::Transcript,
            ]),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        crate::fts_index::search_page(&fts.searcher(), fts.fields(), &query, 0, 10)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn derived_text_pending_revision_ack_handles_update_rename_removal_and_owned_ranges() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("owned");
    std::fs::create_dir(&root).unwrap();
    let path = root.join("voice.mp3");
    let db = crate::transcript_db::TranscriptDb::open_at(dir.path().join("transcript.db")).unwrap();
    let key = save(&db, &path, Some("first"));
    save(&db, &dir.path().join("outside.mp3"), Some("outside"));
    let ranges = crate::metadata_ownership::OwnedRange {
        root: crate::metadata_ownership::root_key(&root),
        exclusions: vec![],
    }
    .sql_ranges();
    let pending = PendingStore::open(
        DerivedTextKind::Transcript,
        &dir.path().join("transcript.db"),
    )
    .unwrap();
    let first = pending.pending_in_ranges(&ranges, 500).unwrap();
    assert_eq!(first.len(), 1);
    save(&db, &path, Some("revised"));
    pending.mark_reflected(&first).unwrap();
    assert!(!db.get(&key).unwrap().unwrap().indexed);
    let second = pending.pending_in_ranges(&ranges, 500).unwrap();
    pending.mark_reflected(&second).unwrap();
    assert!(db.get(&key).unwrap().unwrap().indexed);
    let renamed = crate::transcript_db::TranscriptDb::normalize_path(&root.join("renamed.mp3"));
    let conn = rusqlite::Connection::open(dir.path().join("transcript.db")).unwrap();
    conn.execute(
        "UPDATE transcripts SET item_key=?1 WHERE item_key=?2",
        params![renamed, key],
    )
    .unwrap();
    let changes = pending.pending_in_ranges(&ranges, 500).unwrap();
    assert!(changes.iter().any(|row| row.0 == key && row.2));
    assert!(changes.iter().any(|row| row.0 == renamed && !row.2));
    pending.mark_reflected(&changes).unwrap();
    db.delete(&renamed).unwrap();
    let changes = pending.pending_in_ranges(&ranges, 500).unwrap();
    assert_eq!(changes.len(), 1);
    assert!(changes[0].2);
    pending.mark_reflected(&changes).unwrap();
    assert!(pending.pending_in_ranges(&ranges, 500).unwrap().is_empty());
    assert!(
        !db.get(&crate::transcript_db::TranscriptDb::normalize_path(
            &dir.path().join("outside.mp3")
        ))
        .unwrap()
        .unwrap()
        .indexed
    );
}

#[test]
fn derived_text_descriptors_and_readers_are_the_same_registered_store_list() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DerivedTextPaths::at(dir.path());
    assert_eq!(stores().count(), 2);
    for (kind, path) in paths.iter() {
        assert_eq!(
            kind.descriptor().file,
            path.file_name().unwrap().to_str().unwrap()
        );
        assert!(Reader::open(kind, path).unwrap().is_none());
        assert!(!path.exists());
        let mut effects = crate::rename_key_migration::StoreMutationEffects::default();
        effects.record_rows_changed(kind.descriptor(), 0);
        assert!(!effects.derived_text_changed());
        effects.record_rows_changed(kind.descriptor(), 1);
        assert!(effects.derived_text_changed());
        assert_eq!(
            effects.caption_display_stale(),
            kind == DerivedTextKind::Caption
        );
        assert_eq!(
            effects.transcript_display_stale(),
            kind == DerivedTextKind::Transcript
        );
    }
}

#[test]
#[ignore = "開発用: 1,000時間相当・1,000万文字の合成コーパス。単独で明示実行する"]
fn transcript_synthetic_ten_million_chars_ingest_perf() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("media");
    std::fs::create_dir(&root).unwrap();
    let db = crate::transcript_db::TranscriptDb::open_at(dir.path().join("transcript.db")).unwrap();
    let mut seed = 7_u32;
    let mut paths = Vec::new();
    for index in 0..1000 {
        let text: String = (0..10_000)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                char::from_u32(0x4e00 + ((seed >> 16) % 512)).unwrap()
            })
            .collect();
        let path = root.join(format!("hour-{index:04}.mp3"));
        save(&db, &path, Some(&text));
        paths.push(path);
    }
    let meta = crate::fts_meta::FtsMetaDb::open_at(&dir.path().join("meta.db")).unwrap();
    let index = dir.path().join("index");
    let fts = Arc::new(crate::fts_index::FtsIndex::open_at(&index).unwrap());
    let writer = crate::fts_writer_dispatcher::FtsWriterDispatcher::start(
        fts.writer().unwrap(),
        fts.clone(),
    );
    let session = crate::ingest_worker::IngestSession::new_with_derived_text_paths(
        uuid::Uuid::new_v4(),
        root.clone(),
        &meta,
        &fts,
        DerivedTextPaths::at(dir.path()),
    );
    let candidates = paths.iter().map(|path| candidate(path)).collect();
    let start = std::time::Instant::now();
    let stats = session
        .apply(
            candidates,
            vec![],
            &writer,
            &crate::io_semaphore::GlobalIoSemaphore::new(1),
            crate::io_semaphore::IoPriority::Low,
            &AtomicBool::new(false),
            None,
        )
        .unwrap();
    let seconds = start.elapsed().as_secs_f64();
    assert_eq!(stats.ingested_ok, 1000);
    assert_eq!(stats.derived_text_read_errors, 0);
    drop(session);
    drop(writer);
    drop(fts);
    fn size(path: &Path) -> u64 {
        std::fs::read_dir(path)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    size(&path)
                } else {
                    std::fs::metadata(path).unwrap().len()
                }
            })
            .sum()
    }
    println!(
        "transcript_synthetic_perf {}",
        serde_json::json!({"hours":1000,"documents":1000,"characters":10_000_000,
        "text_utf8_bytes":30_000_000,"ingest_seconds":seconds,"tantivy_bytes":size(&index),"transcript_db_bytes":std::fs::metadata(dir.path().join("transcript.db")).unwrap().len()})
    );
}
