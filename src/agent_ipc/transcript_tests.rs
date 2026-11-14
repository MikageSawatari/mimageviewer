use super::*;
use crate::agent_ipc::tests::Fixture;
use crate::transcript_db::{
    RemovedCounts, SaveOutcome, SaveRequest, TranscriptResult, TranscriptSegment,
};
use std::path::{Path, PathBuf};

fn set_transcripts(allowed: bool) {
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_show_transcripts = allowed;
        db.save_full(&settings)
    })
    .unwrap();
}

fn toggle_gps() {
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.agent_show_gps = !settings.agent_show_gps;
        db.save_full(&settings)
    })
    .unwrap();
}

fn media(fixture: &Fixture, name: &str) -> PathBuf {
    let path = fixture._dir.path().join(name);
    std::fs::write(&path, b"media fixture").unwrap();
    path
}

fn item(path: &Path) -> ItemRef {
    ItemRef {
        path: path.to_string_lossy().into_owned(),
        subresource: None,
    }
}

fn save(path: &Path, result: TranscriptResult) {
    #[cfg(windows)]
    let config_version = crate::ai::transcript_job_worker::CONFIG_VERSION;
    #[cfg(not(windows))]
    let config_version = 1;
    assert_eq!(
        TranscriptDb::open()
            .unwrap()
            .save(&SaveRequest {
                item_path: path.to_owned(),
                signature: FileSignature::from_metadata(&std::fs::metadata(path).unwrap()).unwrap(),
                model_id: "whisper@fixture".into(),
                config_version,
                lang: "ja".into(),
                result,
            })
            .unwrap(),
        SaveOutcome::Written
    );
}

fn save_segments(path: &Path, segments: Vec<TranscriptSegment>) {
    let duration_ms = segments.last().map_or(0, |segment| segment.1);
    save(
        path,
        TranscriptResult::Ok {
            segments,
            removed_counts: RemovedCounts::default(),
            duration_ms,
            audio_stream: 2,
        },
    );
}

fn long_segments() -> Vec<TranscriptSegment> {
    (0..20)
        .map(|index| {
            (
                index * 1000,
                (index + 1) * 1000,
                format!("segment-{index}:{}", "声😀\\\"".repeat(300)),
            )
        })
        .collect()
}

fn context(fixture: &Fixture) -> ReadContext {
    ReadContext::new(
        &fixture.handlers,
        permission().unwrap(),
        Arc::new(AtomicBool::new(false)),
    )
}

fn read(
    fixture: &Fixture,
    item: ItemRef,
    start: Option<f64>,
    cursor: Option<String>,
) -> Result<TranscriptReadResult, AgentError> {
    fixture
        .handlers
        .get_transcript(item, start, cursor, &context(fixture))
}

fn present(result: TranscriptReadResult) -> TranscriptPage {
    let TranscriptReadResult::Present(page) = result else {
        panic!("expected transcript page")
    };
    page
}

#[test]
fn transcript_off_rejects_admission_and_direct_handler_before_opening_corrupt_db() {
    let mut fixture = Fixture::new();
    let path = media(&fixture, "voice.mp4");
    let db_path = TranscriptDb::db_path();
    let corrupt = b"corrupt transcript store";
    std::fs::write(&db_path, corrupt).unwrap();
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(Instant::now),
        execute: Box::new(|_, _| panic!("OFF must reject before starting a response worker")),
    }));
    let connection = fixture.connection();
    let (tx, rx) = crossbeam_channel::bounded(1);
    assert!(
        connection
            .request(
                Request {
                    id: 1,
                    command: Command::GetTranscript {
                        item: item(&path),
                        start_secs: None,
                        cursor: None
                    },
                },
                tx
            )
            .is_none()
    );
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10))
            .unwrap()
            .wire()
            .result
            .unwrap_err()
            .code,
        ErrorCode::Disabled
    );
    assert_eq!(
        read(&fixture, item(&path), None, None).unwrap_err().code,
        ErrorCode::Disabled
    );
    assert_eq!(std::fs::read(&db_path).unwrap(), corrupt);
    set_transcripts(true);
    assert_eq!(
        read(&fixture, item(&path), None, None).unwrap_err().code,
        ErrorCode::Internal
    );
    assert_eq!(std::fs::read(&db_path).unwrap(), corrupt);
}

#[test]
fn transcript_missing_failed_and_non_media_are_distinct_without_creating_store() {
    let fixture = Fixture::new();
    set_transcripts(true);
    let path = media(&fixture, "voice.mp3");
    assert_eq!(
        read(&fixture, item(&path), None, None).unwrap(),
        TranscriptReadResult::None
    );
    assert!(!TranscriptDb::db_path().exists());
    save(&path, TranscriptResult::Failed);
    assert_eq!(
        read(&fixture, item(&path), None, None).unwrap(),
        TranscriptReadResult::Failed
    );
    save_segments(&path, vec![]);
    let empty = present(read(&fixture, item(&path), None, None).unwrap());
    assert_eq!(empty.info.segment_count, 0);
    assert_eq!(empty.info.char_count, 0);
    assert!(empty.segments.is_empty() && empty.cursor.is_none() && !empty.truncated);
    drop(TranscriptDb::open().unwrap());
    std::fs::write(
        TranscriptDb::db_path(),
        b"corrupt non-media store must not be read",
    )
    .unwrap();
    let folder = fixture._dir.path().join("folder.mp4");
    std::fs::create_dir(&folder).unwrap();
    let partial = ItemRef {
        subresource: Some("page_0".into()),
        ..item(&path)
    };
    for input in [
        item(&media(&fixture, "image.png")),
        item(&media(&fixture, "book.zip")),
        item(&media(&fixture, "book.pdf")),
        item(&folder),
        partial,
    ] {
        assert_eq!(
            read(&fixture, input, None, None).unwrap(),
            TranscriptReadResult::NotApplicable
        );
    }
}

#[test]
fn transcript_connection_routes_present_pages_and_start_seconds_keep_overlapping_segment() {
    let fixture = Fixture::new();
    set_transcripts(true);
    let path = media(&fixture, "voice.mp4");
    let segments = vec![
        (0, 1000, "最初".into()),
        (1000, 2000, "声😀".into()),
        (3000, 4500, "最後".into()),
    ];
    save_segments(&path, segments);
    let connection = fixture.connection();
    let Payload::Transcript(result) = fixture
        .call(
            &connection,
            1,
            Command::GetTranscript {
                item: item(&path),
                start_secs: Some(1.5),
                cursor: None,
            },
        )
        .result
        .unwrap()
    else {
        panic!("wrong payload route")
    };
    let page = present(result);
    assert_eq!(page.info.model, "whisper@fixture");
    assert!(page.info.created_at > 0);
    assert_eq!(page.info.audio_stream, Some(2));
    assert_eq!(page.info.duration_secs, 4.5);
    assert_eq!(page.info.segment_count, 3);
    assert_eq!(page.info.char_count, 6);
    assert!(!page.info.stale);
    assert_eq!(page.segments.len(), 2);
    assert_eq!(
        (page.segments[0].start_secs, page.segments[0].end_secs),
        (1.0, 2.0)
    );
    assert_eq!(page.segments[0].text.text, "声😀");
    assert_eq!(
        (page.segments[1].start_secs, page.segments[1].end_secs),
        (3.0, 4.5)
    );
    assert!(page.cursor.is_none() && !page.truncated);
    let after_end = present(read(&fixture, item(&path), Some(4.5), None).unwrap());
    assert!(after_end.segments.is_empty() && after_end.cursor.is_none());
    for invalid in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            read(&fixture, item(&path), Some(invalid), None)
                .unwrap_err()
                .code,
            ErrorCode::BadRequest
        );
    }
}

#[test]
fn transcript_continuations_preserve_start_and_obey_item_and_page_budgets() {
    let fixture = Fixture::new();
    set_transcripts(true);
    let path = media(&fixture, "long.mp4");
    let segments = long_segments();
    save_segments(&path, segments.clone());
    let database_before = std::fs::read(TranscriptDb::db_path()).unwrap();
    let mut page = present(read(&fixture, item(&path), Some(2.5), None).unwrap());
    let info = page.info.clone();
    let mut actual = Vec::new();
    let mut pages = 0;
    loop {
        assert!(!page.segments.is_empty());
        assert_eq!(page.info, info);
        assert!(
            mcp_content_chars(&TranscriptReadResult::Present(page.clone())).unwrap()
                <= MAX_PAGE_CHARS
        );
        for segment in &page.segments {
            assert!(mcp_content_chars(segment).unwrap() <= MAX_ITEM_CHARS);
            assert!(!segment.text.truncated);
            actual.push((
                segment.start_secs,
                segment.end_secs,
                segment.text.text.clone(),
            ));
        }
        pages += 1;
        let Some(cursor) = page.cursor.take() else {
            assert!(!page.truncated);
            break;
        };
        assert!(page.truncated);
        // Omitted start uses the value bound into the first request's cursor.
        page = present(read(&fixture, item(&path), None, Some(cursor)).unwrap());
    }
    assert!(pages > 1);
    let expected: Vec<_> = segments
        .into_iter()
        .skip(2)
        .map(|(start, end, text)| (start as f64 / 1000.0, end as f64 / 1000.0, text))
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(
        std::fs::read(TranscriptDb::db_path()).unwrap(),
        database_before
    );
}

#[test]
fn transcript_deadline_returns_committed_prefix_and_continuation_advances() {
    let mut fixture = Fixture::new();
    set_transcripts(true);
    let path = media(&fixture, "deadline.mp4");
    save_segments(
        &path,
        vec![
            (0, 1000, "最初".into()),
            (1000, 2000, "続き".into()),
            (2000, 3000, "最後".into()),
        ],
    );
    let base = Instant::now();
    let clock_calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&clock_calls);
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(move || {
            if observed_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                base
            } else {
                base + Duration::from_secs(21)
            }
        }),
        execute: Box::new(|_, _| panic!("the test must call the real transcript handler")),
    }));
    let ctx = context(&fixture);
    assert_eq!(clock_calls.load(Ordering::SeqCst), 1);
    let prefix = present(
        fixture
            .handlers
            .get_transcript(item(&path), None, None, &ctx)
            .unwrap(),
    );
    assert_eq!(clock_calls.load(Ordering::SeqCst), 2);
    assert_eq!(prefix.segments.len(), 1);
    assert_eq!(prefix.segments[0].text.text, "最初");
    assert_eq!(prefix.segments[0].start_secs, 0.0);
    assert!(!prefix.segments[0].text.truncated);
    assert!(prefix.truncated);
    let cursor = prefix.cursor.unwrap();
    let data: TranscriptCursor = serde_json::from_slice(&unhex(&cursor).unwrap()).unwrap();
    assert_eq!(data.next, 1);
    assert_eq!(encode(data).unwrap(), cursor);
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = None;
    let continuation = present(read(&fixture, item(&path), None, Some(cursor)).unwrap());
    assert_eq!(continuation.info, prefix.info);
    assert_eq!(
        continuation
            .segments
            .iter()
            .map(|segment| segment.text.text.as_str())
            .collect::<Vec<_>>(),
        ["続き", "最後"]
    );
    assert_eq!(continuation.segments[0].start_secs, 1.0);
    assert!(continuation.cursor.is_none() && !continuation.truncated);
}

#[test]
fn transcript_cursor_rejects_rewritten_deleted_rows_and_current_permission_epoch() {
    let fixture = Fixture::new();
    set_transcripts(true);
    let path = media(&fixture, "long.mp4");
    let segments = long_segments();
    save_segments(&path, segments.clone());
    let first = present(read(&fixture, item(&path), None, None).unwrap());
    let cursor = first.cursor.unwrap();
    let db = TranscriptDb::open().unwrap();
    let old_rev = db
        .get(&TranscriptDb::normalize_path(&path))
        .unwrap()
        .unwrap()
        .rev;
    save_segments(&path, segments.clone());
    assert!(
        db.get(&TranscriptDb::normalize_path(&path))
            .unwrap()
            .unwrap()
            .rev
            > old_rev
    );
    assert_eq!(
        read(&fixture, item(&path), None, Some(cursor))
            .unwrap_err()
            .code,
        ErrorCode::BadRequest
    );
    let cursor = present(read(&fixture, item(&path), None, None).unwrap())
        .cursor
        .unwrap();
    assert_eq!(db.delete(&TranscriptDb::normalize_path(&path)).unwrap(), 1);
    assert_eq!(
        read(&fixture, item(&path), None, Some(cursor))
            .unwrap_err()
            .code,
        ErrorCode::BadRequest
    );
    save_segments(&path, segments);
    let cursor = present(read(&fixture, item(&path), None, None).unwrap())
        .cursor
        .unwrap();
    let previous_epoch = permission().unwrap().1;
    toggle_gps();
    assert!(permission().unwrap().0.transcripts);
    assert_ne!(permission().unwrap().1, previous_epoch);
    assert_eq!(
        read(&fixture, item(&path), None, Some(cursor))
            .unwrap_err()
            .code,
        ErrorCode::PermissionChanged
    );
}

#[test]
fn transcript_cursor_checks_arguments_revision_epoch_and_position_without_process_key() {
    let fixture = Fixture::new();
    set_transcripts(true);
    let path = media(&fixture, "long.mp4");
    let other = media(&fixture, "other.mp4");
    save_segments(&path, long_segments());
    save_segments(&other, long_segments());
    let cursor = present(read(&fixture, item(&path), Some(2.5), None).unwrap())
        .cursor
        .unwrap();
    assert!(matches!(
        read(&fixture, item(&path), Some(2.5), Some(cursor.clone())).unwrap(),
        TranscriptReadResult::Present(_)
    ));
    assert_eq!(
        read(&fixture, item(&path), Some(0.0), Some(cursor.clone()))
            .unwrap_err()
            .code,
        ErrorCode::BadRequest
    );
    assert_eq!(
        read(&fixture, item(&other), None, Some(cursor.clone()))
            .unwrap_err()
            .code,
        ErrorCode::BadRequest
    );
    for (field, replacement, expected) in [
        ("next", serde_json::json!(0), ErrorCode::BadRequest),
        ("next", serde_json::json!(20), ErrorCode::BadRequest),
        ("next", serde_json::json!(usize::MAX), ErrorCode::BadRequest),
        ("rev", serde_json::json!(0), ErrorCode::BadRequest),
        ("rev", serde_json::json!(i64::MAX), ErrorCode::BadRequest),
        ("start_secs", serde_json::json!(3.0), ErrorCode::BadRequest),
        ("start_secs", serde_json::json!(-1.0), ErrorCode::BadRequest),
        (
            "args_digest",
            serde_json::json!("other"),
            ErrorCode::BadRequest,
        ),
        (
            "tool",
            serde_json::json!("list_folder"),
            ErrorCode::BadRequest,
        ),
        ("version", serde_json::json!(2), ErrorCode::BadRequest),
        (
            "permission_epoch",
            serde_json::json!(u64::MAX),
            ErrorCode::PermissionChanged,
        ),
    ] {
        let mut encoded: serde_json::Value =
            serde_json::from_slice(&unhex(&cursor).unwrap()).unwrap();
        encoded[field] = replacement;
        let forged = hex(&serde_json::to_vec(&encoded).unwrap());
        assert_eq!(
            read(&fixture, item(&path), None, Some(forged))
                .unwrap_err()
                .code,
            expected,
            "{field}"
        );
    }
    // 許可された同じ本文内の別位置は認可を広げない。鍵や台帳を使わず読み直せる。
    let mut data: TranscriptCursor = serde_json::from_slice(&unhex(&cursor).unwrap()).unwrap();
    data.next = 19;
    let page = present(read(&fixture, item(&path), None, Some(encode(data).unwrap())).unwrap());
    assert_eq!(page.segments.len(), 1);
    assert!(page.segments[0].text.text.starts_with("segment-19:"));
    for malformed in [String::new(), "0".into(), "zz".into(), "0".repeat(8194)] {
        assert_eq!(
            read(&fixture, item(&path), None, Some(malformed))
                .unwrap_err()
                .code,
            ErrorCode::BadRequest
        );
    }
}

#[test]
fn transcript_giant_escaped_utf8_segment_is_marked_and_cursor_advances_to_next_segment() {
    let fixture = Fixture::new();
    set_transcripts(true);
    let path = media(&fixture, "giant.mp4");
    let giant = "声😀\\\"\n".repeat(20_000);
    let mut segments = vec![(0, 1000, giant.clone())];
    segments.extend((1..12).map(|index| {
        (
            index * 1000,
            (index + 1) * 1000,
            format!("after-{index}:{}", "x".repeat(4500)),
        )
    }));
    save_segments(&path, segments.clone());
    let mut page = present(read(&fixture, item(&path), None, None).unwrap());
    let first = &page.segments[0];
    assert_eq!((first.start_secs, first.end_secs), (0.0, 1.0));
    assert!(first.text.truncated);
    assert!(!first.text.text.is_empty());
    assert!(giant.starts_with(&first.text.text));
    assert!(first.text.text.len() < giant.len());
    let mut actual = Vec::new();
    loop {
        assert!(
            mcp_content_chars(&TranscriptReadResult::Present(page.clone())).unwrap()
                <= MAX_PAGE_CHARS
        );
        for segment in &page.segments {
            assert!(mcp_content_chars(segment).unwrap() <= MAX_ITEM_CHARS);
            actual.push((
                segment.start_secs,
                segment.text.truncated,
                segment.text.text.clone(),
            ));
        }
        let Some(cursor) = page.cursor.take() else {
            break;
        };
        page = present(read(&fixture, item(&path), None, Some(cursor)).unwrap());
    }
    assert_eq!(actual.len(), segments.len());
    assert!(actual[0].1);
    for (actual, (start, _, text)) in actual.iter().skip(1).zip(segments.iter().skip(1)) {
        assert_eq!(actual, &(*start as f64 / 1000.0, false, text.clone()));
    }
    // A one-segment result keeps the truncation mark even without a cursor.
    save_segments(&path, vec![(0, 1000, giant)]);
    let only = present(read(&fixture, item(&path), None, None).unwrap());
    assert!(only.truncated && only.cursor.is_none() && only.segments[0].text.truncated);
}

#[test]
fn transcript_reply_is_withheld_when_epoch_changes_before_wire_send() {
    let fixture = Fixture::new();
    set_transcripts(true);
    let path = media(&fixture, "voice.mp4");
    save_segments(&path, vec![(0, 1000, "PRIVATE conversation".into())]);
    let connection = fixture.connection();
    let (tx, rx) = crossbeam_channel::bounded(1);
    let worker = connection
        .request(
            Request {
                id: 1,
                command: Command::GetTranscript {
                    item: item(&path),
                    start_secs: None,
                    cursor: None,
                },
            },
            tx,
        )
        .unwrap();
    let reply = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    assert!(matches!(
        &reply.response.result,
        Ok(Payload::Transcript(TranscriptReadResult::Present(_)))
    ));
    toggle_gps();
    assert!(permission().unwrap().0.transcripts);
    let sent = reply.wire();
    assert_eq!(sent.result.unwrap_err().code, ErrorCode::PermissionChanged);
}
