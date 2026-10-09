use super::*;
use std::io::{BufRead, Cursor, Read};

fn configured(root: &Path, records: usize, bytes: usize) -> Arc<Service> {
    let service = Service::new(records, bytes);
    service
        .normal
        .set(NormalConfig {
            path: root.join("mimageviewer.log"),
            truncate: true,
        })
        .ok();
    service.perf.set(PerfConfig { path: root.join("perf_events.jsonl"), rotate: false, start: service.clock, session: json!({"t":0,"cat":"session","kind":"start","pid":std::process::id(),"session_id":service.session_id,"log_delivery_schema":1}) }).ok();
    service
}

#[track_caller]
fn submit(service: &Arc<Service>, sink: Sink, text: &str) -> bool {
    service.publication(sink).is_some_and(|publication| {
        publication.submit(
            text.into(),
            service.clock(),
            17,
            std::panic::Location::caller(),
        )
    })
}

fn rows(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn snapshot_text(snapshot: SnapshotFile) -> String {
    let mut text = String::new();
    snapshot
        .file
        .take(snapshot.len)
        .read_to_string(&mut text)
        .unwrap();
    text
}

#[test]
fn record_and_payload_limits_drop_new_and_preserve_adopted_fifo() {
    let directory = tempfile::tempdir().unwrap();
    let service = configured(directory.path(), 2, 16);
    assert!(submit(&service, Sink::Normal, "first\n"));
    assert!(submit(&service, Sink::Perf, "second\n"));
    assert!(!submit(&service, Sink::Normal, "third\n"));
    assert_eq!(service.records.load(Ordering::Acquire), 2);
    assert_eq!(service.bytes.load(Ordering::Acquire), 13);
    assert_eq!(service.drops[0][0].count.load(Ordering::Acquire), 1);
    let queue = service.queue.lock().unwrap();
    assert_eq!(queue[0].line, "first\n");
    assert_eq!(queue[1].line, "second\n");
    drop(queue);
    let service = configured(directory.path(), 8, 5);
    assert!(submit(&service, Sink::Normal, "1234"));
    assert!(!submit(&service, Sink::Perf, "12"));
    assert_eq!(service.bytes.load(Ordering::Acquire), 4);
    assert_eq!(service.records.load(Ordering::Acquire), 1);
    assert_eq!(service.drops[1][1].count.load(Ordering::Acquire), 1);
}

#[test]
fn oversize_and_journal_contention_restore_reservations_without_waiting() {
    let directory = tempfile::tempdir().unwrap();
    let service = configured(directory.path(), 8, 2 * RECORD_BYTES);
    assert!(!submit(
        &service,
        Sink::Normal,
        &"x".repeat(RECORD_BYTES + 1)
    ));
    assert_eq!(service.drops[0][2].count.load(Ordering::Acquire), 1);
    let queue = service.queue.lock().unwrap();
    assert!(!submit(&service, Sink::Perf, "contended"));
    assert_eq!(service.drops[1][3].count.load(Ordering::Acquire), 1);
    assert_eq!(service.records.load(Ordering::Acquire), 0);
    assert_eq!(service.bytes.load(Ordering::Acquire), 0);
    drop(queue);
    assert!(submit(&service, Sink::Normal, "resumed\n"));
}

fn producers_finish_while_stalled(service: &Arc<Service>) {
    let (finished, receiver) = mpsc::channel();
    let producer = service.clone();
    let first = finished.clone();
    let regular = std::thread::spawn(move || {
        for sequence in 0..64 {
            assert!(submit(
                &producer,
                Sink::Normal,
                &format!("regular-{sequence}\n")
            ));
            assert!(submit(
                &producer,
                Sink::Perf,
                &format!("{{\"kind\":\"producer\",\"seq\":{sequence}}}\n")
            ));
        }
        first.send(()).unwrap();
    });
    let reader = service.clone();
    let stderr = std::thread::spawn(move || {
        // A diagnostic stderr pump must keep draining its IPC source even
        // when both logging destinations share a stalled file owner.
        for line in std::io::BufReader::new(Cursor::new("stderr diagnostic\n".repeat(64))).lines() {
            assert!(submit(
                &reader,
                Sink::Normal,
                &format!("stderr: {}\n", line.unwrap())
            ));
        }
        finished.send(()).unwrap();
    });
    receiver.recv_timeout(BARRIER_WAIT).unwrap();
    receiver.recv_timeout(BARRIER_WAIT).unwrap();
    regular.join().unwrap();
    stderr.join().unwrap();
}

#[test]
fn real_worker_open_stall_does_not_block_producer_or_stderr_pump() {
    let directory = tempfile::tempdir().unwrap();
    let service = configured(directory.path(), 1024, 65536);
    let pause = test_support::pause_service(&service, "open");
    service.start();
    pause.entered();
    producers_finish_while_stalled(&service);
    assert!(!directory.path().join("mimageviewer.log").exists());
    drop(pause);
    assert!(service.shutdown(Instant::now() + BARRIER_WAIT));
    assert!(
        std::fs::read_to_string(directory.path().join("mimageviewer.log"))
            .unwrap()
            .contains("regular-63")
    );
}

fn worker_stall(stage: &'static str) {
    let directory = tempfile::tempdir().unwrap();
    let writer = test_support::writer(directory.path(), 1024, 65536);
    let pause = writer.pause(stage);
    assert!(writer.submit_normal("trigger stalled sink"));
    pause.entered();
    producers_finish_while_stalled(&writer.service);
    drop(pause);
    let delivered = writer.snapshot().unwrap();
    let text = snapshot_text(delivered.normal);
    assert!(text.contains("trigger stalled sink"));
    assert!(text.contains("regular-63"));
    assert!(text.contains("stderr: stderr diagnostic"));
    assert!(writer.shutdown());
}

#[test]
fn real_worker_write_stall_does_not_block_producer_or_stderr_pump() {
    worker_stall("write");
}

#[test]
fn real_worker_flush_stall_does_not_block_producer_or_stderr_pump() {
    worker_stall("flush");
}

#[test]
fn rotation_stall_and_snapshot_handle_preserve_saved_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let service = configured(directory.path(), 1024, 65536);
    let mut worker = Worker::new(service.clone());
    worker.initialize();
    let FileSink::Ready(normal) = &mut worker.normal else {
        panic!("normal unavailable")
    };
    normal.set_test_retention(16, 1);
    worker.raw(Sink::Normal, "saved-prefix\n");
    worker.flush(false);
    let saved = snapshot(&directory.path().join("mimageviewer.log")).unwrap();
    let unrelated = directory.path().join("settings.db");
    std::fs::write(&unrelated, b"user evidence").unwrap();
    let pause = test_support::pause_service(&service, "rotation");
    assert!(submit(&service, Sink::Normal, "rotated-tail\n"));
    let thread = std::thread::spawn(move || worker.run());
    pause.entered();
    producers_finish_while_stalled(&service);
    drop(pause);
    assert!(service.shutdown(Instant::now() + BARRIER_WAIT));
    thread.join().unwrap();
    assert_eq!(snapshot_text(saved), "saved-prefix\n");
    assert_eq!(std::fs::read(&unrelated).unwrap(), b"user evidence");
    assert!(directory.path().join("mimageviewer.log.bak").exists());
}

#[test]
fn multithreaded_producer_timestamps_tids_and_each_thread_order_survive_delivery() {
    let directory = tempfile::tempdir().unwrap();
    let writer = test_support::writer(directory.path(), 1024, 131072);
    let mut threads = Vec::new();
    for producer in 0..4u64 {
        let service = writer.service.clone();
        threads.push(std::thread::spawn(move || {
            for sequence in 0..32 {
                let stamp = 100.0 + producer as f64 + sequence as f64 / 1000.0;
                let event = json!({"t":stamp,"tid":producer+500,"kind":"ordered","producer":producer,"seq":sequence});
                let publication = service.publication(Sink::Perf).unwrap();
                assert!(publication.submit(format!("{event}\n"), stamp, producer+500, std::panic::Location::caller()));
            }
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }
    let delivered = writer.snapshot().unwrap();
    assert_eq!(delivered.checkpoint.perf_dropped_total, 0);
    let perf = snapshot_text(delivered.perf.unwrap());
    let ordered = perf
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["kind"] == "ordered")
        .collect::<Vec<_>>();
    assert_eq!(ordered.len(), 128);
    for producer in 0..4u64 {
        let own = ordered
            .iter()
            .filter(|event| event["producer"] == producer)
            .collect::<Vec<_>>();
        for (sequence, event) in own.iter().enumerate() {
            assert_eq!(event["seq"], sequence);
            assert_eq!(event["tid"], producer + 500);
            assert_eq!(
                event["t"],
                100.0 + producer as f64 + sequence as f64 / 1000.0
            );
        }
    }
    assert!(writer.shutdown());
}

#[test]
fn summaries_write_delta_total_and_time_span_without_reentering_full_queue() {
    let directory = tempfile::tempdir().unwrap();
    let service = configured(directory.path(), 1, 4096);
    let mut worker = Worker::new(service.clone());
    worker.initialize();
    assert!(submit(&service, Sink::Normal, "adopted\n"));
    service.drops[0][0].note(7.5);
    service.drops[0][0].note(3.5);
    worker.drop_summaries();
    worker.flush(true);
    assert_eq!(service.records.load(Ordering::Acquire), 1);
    assert_eq!(
        service.queue.lock().unwrap().front().unwrap().line,
        "adopted\n"
    );
    service.drops[0][0].note(9.5);
    worker.drop_summaries();
    worker.drop_summaries();
    worker.flush(true);
    let summaries = rows(&directory.path().join("perf_events.jsonl"))
        .into_iter()
        .filter(|event| event["kind"] == "records_dropped")
        .collect::<Vec<_>>();
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0]["count"], 2);
    assert_eq!(summaries[0]["total"], 2);
    assert_eq!(summaries[0]["first_t"], 3.5);
    assert_eq!(summaries[0]["last_t"], 7.5);
    assert_eq!(summaries[1]["count"], 1);
    assert_eq!(summaries[1]["total"], 3);
    assert_eq!(summaries[1]["last_t"], 9.5);
    assert!(
        std::fs::read_to_string(directory.path().join("mimageviewer.log"))
            .unwrap()
            .contains("count=1 total=3")
    );
}

#[test]
fn registered_publication_prevents_premature_barrier_ack_and_busy_request() {
    let directory = tempfile::tempdir().unwrap();
    let writer = test_support::writer(directory.path(), 32, 65536);
    // Registration precedes the producer's timestamp/formatting boundary.
    let publication = writer.service.publication(Sink::Normal).unwrap();
    let cutoff = publication.epoch;
    let pause = writer.pause("control");
    let service = writer.service.clone();
    let (finished, receiver) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        finished
            .send(service.barrier("registered-producer".into(), "test"))
            .unwrap();
    });
    pause.entered();
    assert!(writer.service.barrier("duplicate".into(), "test").is_err());
    drop(pause);
    assert!(receiver.recv_timeout(Duration::from_millis(30)).is_err());
    assert!(publication.submit(
        "registered tail\n".into(),
        writer.service.clock(),
        808,
        std::panic::Location::caller()
    ));
    let delivered = receiver.recv_timeout(BARRIER_WAIT).unwrap().unwrap();
    assert_eq!(delivered.checkpoint.ticket_cutoff, cutoff);
    assert_eq!(delivered.checkpoint.settled_watermark, cutoff);
    assert!(snapshot_text(delivered.normal.unwrap()).contains("registered tail"));
    waiter.join().unwrap();
    assert!(writer.shutdown());
}

#[test]
fn normal_tail_flushes_inside_batch_even_when_periodic_clock_cannot_fire() {
    let directory = tempfile::tempdir().unwrap();
    let service = configured(directory.path(), 32, 65536);
    let mut worker = Worker::new(service.clone());
    worker.initialize();
    worker.last_flush = Instant::now() + Duration::from_secs(60);
    assert!(submit(&service, Sink::Normal, "last line before crash\n"));
    let pause = test_support::pause_service(&service, "flush");
    let (finished, receiver) = mpsc::channel();
    let batch = std::thread::spawn(move || {
        assert!(worker.batch());
        finished.send(()).unwrap();
    });
    pause.entered();
    assert!(receiver.try_recv().is_err());
    assert!(
        std::fs::read_to_string(directory.path().join("mimageviewer.log"))
            .unwrap()
            .contains("last line before crash")
    );
    drop(pause);
    receiver.recv_timeout(BARRIER_WAIT).unwrap();
    batch.join().unwrap();
}

#[test]
fn perf_short_tail_is_buffered_until_periodic_worker_flush() {
    let directory = tempfile::tempdir().unwrap();
    let service = configured(directory.path(), 32, 65536);
    let mut worker = Worker::new(service.clone());
    worker.initialize();
    assert!(submit(
        &service,
        Sink::Perf,
        "{\"kind\":\"periodic-tail\"}\n"
    ));
    assert!(worker.batch());
    assert!(worker.perf_dirty);
    assert!(
        !rows(&directory.path().join("perf_events.jsonl"))
            .iter()
            .any(|event| event["kind"] == "periodic-tail")
    );
    let FileSink::Ready(perf) = &worker.perf else {
        panic!("perf unavailable")
    };
    assert!(!perf.file.buffer().is_empty());
    worker.last_flush = Instant::now().checked_sub(PERF_FLUSH).unwrap();
    let pause = test_support::pause_service(&service, "flush");
    let thread = std::thread::spawn(move || worker.run());
    pause.entered();
    drop(pause);
    assert!(service.shutdown(Instant::now() + BARRIER_WAIT));
    thread.join().unwrap();
    assert!(
        rows(&directory.path().join("perf_events.jsonl"))
            .iter()
            .any(|event| event["kind"] == "periodic-tail")
    );
}

#[test]
fn one_failed_sink_stays_unavailable_while_other_sink_continues() {
    let directory = tempfile::tempdir().unwrap();
    let service = configured(directory.path(), 32, 65536);
    let bad_path = directory.path().join("mimageviewer.log");
    std::fs::create_dir(&bad_path).unwrap();
    let mut worker = Worker::new(service.clone());
    worker.initialize();
    assert_eq!(worker.normal.status(), "failed");
    assert_eq!(worker.perf.status(), "ready");
    // Making the path usable later must not introduce automatic reopen/retry.
    std::fs::remove_dir(&bad_path).unwrap();
    worker.initialize();
    assert_eq!(worker.normal.status(), "failed");
    assert!(submit(&service, Sink::Normal, "unavailable sink\n"));
    assert!(submit(
        &service,
        Sink::Perf,
        "{\"kind\":\"healthy-sink\"}\n"
    ));
    assert!(worker.batch());
    worker.flush(true);
    assert!(!bad_path.exists());
    assert!(
        rows(&directory.path().join("perf_events.jsonl"))
            .iter()
            .any(|event| event["kind"] == "healthy-sink")
    );
    let (response, _) = mpsc::channel();
    let control = Control {
        request_id: "failed".into(),
        purpose: "test".into(),
        coverage_at: Instant::now(),
        cutoff: service.epoch.load(Ordering::Acquire),
        deadline: Instant::now() + BARRIER_WAIT,
        response,
    };
    assert!(worker.deliver(&control).is_err());
}

#[test]
fn expired_control_never_reports_successful_delivery() {
    let directory = tempfile::tempdir().unwrap();
    let service = configured(directory.path(), 32, 65536);
    let pause = test_support::pause_service(&service, "control");
    let (response, receiver) = mpsc::channel();
    service.control_busy.store(true, Ordering::Release);
    let cutoff = service.epoch.fetch_add(1, Ordering::SeqCst);
    *service.control.lock().unwrap() = Some(Control {
        request_id: "expired".into(),
        purpose: "test".into(),
        coverage_at: Instant::now(),
        cutoff,
        deadline: Instant::now(),
        response,
    });
    service.start();
    pause.entered();
    drop(pause);
    let result = receiver.recv_timeout(BARRIER_WAIT).unwrap();
    assert!(matches!(result, Err(error) if error.contains("deadline")));
    assert!(service.shutdown(Instant::now() + BARRIER_WAIT));
}

#[test]
fn shutdown_deadline_leaves_io_on_worker_then_flushes_tail_and_drop_summary() {
    let directory = tempfile::tempdir().unwrap();
    let writer = test_support::writer(directory.path(), 2, 65536);
    let pause = writer.pause_write();
    assert!(writer.submit_normal("shutdown accepted tail"));
    pause.entered();
    assert!(writer.submit_normal("queued tail"));
    assert!(!writer.submit_normal("drop at capacity"));
    let deadline = Instant::now();
    assert!(!writer.service.shutdown(deadline));
    assert!(!writer.service.shutdown(deadline));
    assert!(!writer.submit_normal("after closing"));
    assert!(
        writer
            .service
            .barrier("after-close".into(), "test")
            .is_err()
    );
    drop(pause);
    assert!(writer.shutdown());
    let normal = std::fs::read_to_string(directory.path().join("mimageviewer.log")).unwrap();
    assert!(normal.contains("shutdown accepted tail"));
    assert!(normal.contains("queued tail"));
    assert!(normal.contains("[async-log] dropped count=1 total=1"));
    assert!(!normal.contains("after closing"));
}

#[test]
fn snapshot_permits_perf_off_but_contains_current_export_marker() {
    let directory = tempfile::tempdir().unwrap();
    let service = Service::new(32, 65536);
    service
        .normal
        .set(NormalConfig {
            path: directory.path().join("normal-only.log"),
            truncate: true,
        })
        .ok();
    service.start();
    assert!(submit(&service, Sink::Normal, "normal only tail\n"));
    let snapshot = service.snapshot().unwrap();
    assert!(snapshot.perf.is_none());
    assert_eq!(snapshot.checkpoint.perf_status, "disabled");
    let text = snapshot_text(snapshot.normal);
    assert!(text.contains("normal only tail"));
    assert!(text.contains("diagnostics: export_requested"));
    assert!(service.shutdown(Instant::now() + BARRIER_WAIT));
}

#[test]
fn late_perf_enable_retains_inflight_barrier_absolute_coverage_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let service = Service::new(32, 65536);
    service
        .normal
        .set(NormalConfig {
            path: directory.path().join("normal.log"),
            truncate: true,
        })
        .ok();
    service.start();
    service.barrier("normal-ready".into(), "test").unwrap();
    let publication = service.publication(Sink::Normal).unwrap();
    let pause = test_support::pause_service(&service, "control");
    let caller = service.clone();
    let (finished, receiver) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        finished
            .send(caller.barrier("before-perf-enabled".into(), "test"))
            .unwrap();
    });
    pause.entered();
    // The barrier's absolute observation boundary is already fixed. Enabling
    // perf now must convert that earlier boundary to zero in the new clock,
    // rather than mistake the old service elapsed time for perf coverage.
    service.perf.set(PerfConfig { path: directory.path().join("late-perf.jsonl"), rotate: false, start: Instant::now(), session: json!({"t":0,"cat":"session","kind":"start","pid":std::process::id(),"session_id":service.session_id,"log_delivery_schema":1}) }).ok();
    drop(pause);
    assert!(publication.submit(
        "before perf clock\n".into(),
        service.clock(),
        71,
        std::panic::Location::caller()
    ));
    let delivered = receiver.recv_timeout(BARRIER_WAIT).unwrap().unwrap();
    assert_eq!(delivered.checkpoint.coverage_t, 0.0);
    assert_eq!(delivered.checkpoint.perf_status, "ready");
    assert!(snapshot_text(delivered.normal.unwrap()).contains("before perf clock"));
    let text = snapshot_text(delivered.perf.unwrap());
    let events = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events[0]["kind"], "start");
    assert!(
        events
            .iter()
            .any(|event| event["kind"] == "drain_checkpoint"
                && event["request_id"] == "before-perf-enabled"
                && event["coverage_t"] == 0.0)
    );
    thread.join().unwrap();
    assert!(service.shutdown(Instant::now() + BARRIER_WAIT));
}

#[test]
fn own_export_marker_drop_refuses_snapshot_without_barrier() {
    let directory = tempfile::tempdir().unwrap();
    let writer = test_support::writer(directory.path(), 1, 65536);
    let pause = writer.pause_write();
    assert!(writer.submit_normal("occupy in-flight record budget"));
    pause.entered();
    assert!(writer.snapshot().is_err());
    assert!(!writer.service.control_busy.load(Ordering::Acquire));
    drop(pause);
    assert!(writer.shutdown());
    let rows = rows(&directory.path().join("perf_events.jsonl"));
    let checkpoint = rows.last().unwrap();
    assert_eq!(checkpoint["purpose"], "shutdown");
    assert!(checkpoint["normal_dropped_total"].as_u64().unwrap() > 0);
}

#[test]
fn shutdown_final_checkpoint_follows_tail_and_drop_summary() {
    let directory = tempfile::tempdir().unwrap();
    let writer = test_support::writer(directory.path(), 32, 65536);
    assert!(writer.submit_perf(json!({"cat":"frame","kind":"last_tail"})));
    writer.force_drop();
    assert!(writer.shutdown());
    let rows = rows(&directory.path().join("perf_events.jsonl"));
    let checkpoint = rows.last().unwrap();
    assert_eq!(checkpoint["kind"], "drain_checkpoint");
    assert_eq!(checkpoint["purpose"], "shutdown");
    assert_eq!(checkpoint["normal_dropped_total"], 1);
    assert_eq!(checkpoint["dropped_by_reason"]["normal"]["oversize"], 1);
    for sink in ["normal", "perf"] {
        let reason_total: u64 = checkpoint["dropped_by_reason"][sink]
            .as_object()
            .unwrap()
            .values()
            .map(|count| count.as_u64().unwrap())
            .sum();
        assert_eq!(checkpoint[format!("{sink}_dropped_total")], reason_total);
    }
    assert!(rows.iter().any(|row| row["kind"] == "last_tail"));
    assert!(rows.iter().any(|row| row["kind"] == "records_dropped"));
    assert!(!writer.submit_normal("after stopped"));
    assert!(writer.shutdown());
}

#[test]
fn full_journal_cannot_drop_protected_session_start() {
    let directory = tempfile::tempdir().unwrap();
    let service = configured(directory.path(), 1, 65536);
    assert!(submit(
        &service,
        Sink::Normal,
        "full before file initialization\n"
    ));
    assert!(!submit(&service, Sink::Perf, "{\"kind\":\"overflow\"}\n"));
    service.start();
    assert!(service.shutdown(Instant::now() + BARRIER_WAIT));
    let rows = rows(&directory.path().join("perf_events.jsonl"));
    assert_eq!(rows.first().unwrap()["kind"], "start");
    assert_eq!(rows.last().unwrap()["perf_dropped_total"], 1);
}

#[test]
fn slow_open_witness_uses_file_owner_times_without_a_producer_wait() {
    let directory = tempfile::tempdir().unwrap();
    let service = configured(directory.path(), 32, 65536);
    let mut worker = Worker::new(service.clone());
    worker.initialize();
    let started = Instant::now();
    let ended = started + Duration::from_millis(100);
    worker.open_completed(Sink::Normal, started, ended);
    worker.flush(true);
    let normal = std::fs::read_to_string(directory.path().join("mimageviewer.log")).unwrap();
    assert!(normal.contains("[async-log] open sink=normal"));
    assert!(normal.contains("elapsed_ms=100.000 io_owner=writer"));
    let rows = rows(&directory.path().join("perf_events.jsonl"));
    let witness = rows
        .iter()
        .find(|row| {
            row["operation"] == "open" && row["logger"] == "normal" && row["hold_ms"] == 100.0
        })
        .unwrap();
    assert_eq!(witness["io_owner"], "writer");
    assert_eq!(
        witness["tid"],
        crate::logger::current_thread_id_num().unwrap_or(0)
    );
    assert_eq!(witness["hold_ms"], 100.0);
    assert_eq!(witness["auxiliary_ms"], 100.0);
    assert_eq!(witness["wait_ms"], 0.0);
    assert!(witness["end_t"].as_f64().unwrap() > witness["start_t"].as_f64().unwrap());
    // The same variant which distinguishes unopened from unavailable also
    // prevents subsequent config checks from reopening a failed sink.
    worker.failed(Sink::Normal, std::io::Error::other("fixture failure"));
    worker.initialize();
    assert!(matches!(worker.normal, FileSink::Failed));
}

#[test]
fn resumed_batch_reports_drops_before_the_remaining_tail_is_drained() {
    let directory = tempfile::tempdir().unwrap();
    let writer = test_support::writer(directory.path(), 2, 65536);
    let stalled = writer.pause_write();
    assert!(writer.submit_normal("blocked batch"));
    stalled.entered();
    assert!(writer.submit_normal("remaining queued tail"));
    assert!(!writer.submit_normal("drop at capacity"));
    let resumed = writer.pause("resumed-batch");
    drop(stalled);
    resumed.entered();
    let normal = std::fs::read_to_string(directory.path().join("mimageviewer.log")).unwrap();
    assert!(normal.contains("blocked batch"));
    assert!(normal.contains("[async-log] dropped count=1 total=1"));
    assert!(!normal.contains("remaining queued tail"));
    assert_eq!(writer.service.queue.lock().unwrap().len(), 1);
    drop(resumed);
    assert!(writer.shutdown());
}

#[test]
fn rejected_registration_publishes_its_drop_before_settling_the_old_cohort() {
    let directory = tempfile::tempdir().unwrap();
    let writer = test_support::writer(directory.path(), 32, 65536);
    let registered = writer.pause("publication-registered");
    let epoch = writer.service.epoch.load(Ordering::SeqCst);
    let producer = writer.clone();
    let producer = std::thread::spawn(move || producer.submit_normal("crossing the cutoff"));
    registered.entered();
    let control = writer.pause("control");
    let service = writer.service.clone();
    let (completed, result) = mpsc::channel();
    let barrier = std::thread::spawn(move || {
        completed
            .send(service.barrier("registration-cutoff".into(), "test"))
            .unwrap()
    });
    control.entered();
    let rejecting = writer.pause("registration-drop");
    drop(registered);
    rejecting.entered();
    let settled_too_soon = writer.service.active[epoch as usize % 2].load(Ordering::SeqCst) == 0;
    drop(control);
    drop(rejecting);
    assert!(!producer.join().unwrap());
    let delivery = result.recv_timeout(BARRIER_WAIT).unwrap().unwrap();
    barrier.join().unwrap();
    assert!(
        !settled_too_soon,
        "registered drop was declared settled before counter publication"
    );
    assert_eq!(delivery.checkpoint.normal_dropped_total, 1);
    assert_eq!(
        delivery.checkpoint.dropped_by_reason["normal"]["journal_contended"],
        1
    );
    assert!(writer.shutdown());
}
