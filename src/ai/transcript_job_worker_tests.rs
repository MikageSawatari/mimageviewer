use super::*;
use std::time::Duration;

fn config(root: &Path) -> TranscriptWorkerConfig {
    TranscriptWorkerConfig {
        worker_executable: Some(super::super::vlm_worker::tests::fake_worker().to_owned()),
        load: asr_worker::LoadRequest {
            model_path: "model.bin".into(),
            device_luid: "00000000:00000001".into(),
        },
        model_id: "test-model".into(),
        transcript_db_path: root.join("transcript.db"),
    }
}

fn pipes(root: &Path) -> (AsrInput, AsrOutput) {
    let config = config(root);
    let mut client = AsrWorkerClient::start_at(config.worker_executable.as_ref().unwrap()).unwrap();
    client.hello().unwrap();
    client.load(&config.load).unwrap();
    client.split().unwrap()
}

fn wav(path: &Path) {
    let data_len = 3200u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&16000u32.to_le_bytes());
    bytes.extend_from_slice(&32000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    bytes.resize(bytes.len() + data_len as usize, 0);
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn extraction_failure_ends_source_and_reuses_loaded_worker() {
    let dir = tempfile::tempdir().unwrap();
    let (mut input, mut output) = pipes(dir.path());
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, _rx) = mpsc::channel();
    let ctx = egui::Context::default();
    let result = infer(
        &mut input,
        &mut output,
        &cancel,
        1,
        100,
        &tx,
        &ctx,
        |sink| {
            sink(&[0; 3200]).map_err(|e| e.to_string())?;
            Err("synthetic conversion failure".into())
        },
    )
    .unwrap();
    assert_eq!(result.duration_ms, 100);
    assert!(result.extraction_error.is_some());
    // 呼出し元はこの暫定区間を保存しない。同じLoadedモデルで次のbeginを受けられる。
    let result = infer(
        &mut input,
        &mut output,
        &cancel,
        2,
        100,
        &tx,
        &ctx,
        |sink| sink(&[0; 3200]).map_err(|e| e.to_string()),
    )
    .unwrap();
    assert_eq!(result.segments, [(1, 100, "日本語".into())]);
    assert!(result.extraction_error.is_none());
}

#[test]
fn extractor_panic_kills_peer_and_unblocks_result_reader() {
    let dir = tempfile::tempdir().unwrap();
    let (mut input, mut output) = pipes(dir.path());
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, _rx) = mpsc::channel();
    let result = infer(
        &mut input,
        &mut output,
        &cancel,
        1,
        100,
        &tx,
        &egui::Context::default(),
        |_| panic!("synthetic extractor panic"),
    );
    assert!(result.is_err());
    assert!(input.kill_handle().is_killed());
}

#[test]
fn cancellation_before_handle_publication_also_kills_process() {
    let dir = tempfile::tempdir().unwrap();
    let (input, _output) = pipes(dir.path());
    let cancel = Arc::new(AtomicBool::new(false));
    let control = Control {
        cancel,
        kill: Mutex::new(None),
    };
    control.stop();
    let kill = input.kill_handle();
    assert!(!control.publish(kill.clone()));
    assert!(kill.is_killed());
}

#[test]
fn successful_file_is_saved_and_matching_request_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.wav");
    wav(&path);
    let config = config(dir.path());
    let db = TranscriptDb::open_at(&config.transcript_db_path).unwrap();
    let (mut input, mut output) = pipes(dir.path());
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, _rx) = mpsc::channel();
    let ctx = egui::Context::default();
    assert_eq!(
        process_item(
            &config,
            &db,
            &cancel,
            &path.clone().into(),
            1,
            &mut input,
            &mut output,
            &tx,
            &ctx
        )
        .unwrap(),
        TranscriptItemOutcome::Created
    );
    let row = db.get(&path.to_string_lossy()).unwrap().unwrap();
    assert_eq!(row.segments, [(1, 100, "日本語".into())]);
    assert_eq!(row.audio_stream, Some(0));
    assert_eq!(
        process_item(
            &config,
            &db,
            &cancel,
            &path.clone().into(),
            2,
            &mut input,
            &mut output,
            &tx,
            &ctx
        )
        .unwrap(),
        TranscriptItemOutcome::AlreadyCreated
    );
    let mut changed = row.clone();
    let signature = row.signature;
    assert!(reusable(&row, signature, 0, "test-model"));
    changed.status = TranscriptStatus::Failed;
    assert!(!reusable(&changed, signature, 0, "test-model"));
    changed = row.clone();
    changed.config_version += 1;
    assert!(!reusable(&changed, signature, 0, "test-model"));
    assert!(!reusable(&row, signature, 1, "test-model"));
    assert!(!reusable(&row, signature, 0, "other-model"));
    assert!(!reusable(
        &row,
        FileSignature {
            size: signature.size + 1,
            ..signature
        },
        0,
        "test-model"
    ));
}

#[test]
fn failed_input_is_persisted_but_existing_ok_is_retained() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.wav");
    std::fs::write(&path, b"invalid media").unwrap();
    let config = config(dir.path());
    let db = TranscriptDb::open_at(&config.transcript_db_path).unwrap();
    let (mut input, mut output) = pipes(dir.path());
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, _rx) = mpsc::channel();
    let ctx = egui::Context::default();
    assert_eq!(
        process_item(
            &config,
            &db,
            &cancel,
            &path.clone().into(),
            1,
            &mut input,
            &mut output,
            &tx,
            &ctx
        )
        .unwrap(),
        TranscriptItemOutcome::FailedWritten
    );
    let signature = FileSignature::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    assert_eq!(
        save(
            &config,
            &db,
            &cancel,
            &path,
            signature,
            2,
            TranscriptResult::Ok {
                segments: vec![(0, 0, "保存済み".into())],
                duration_ms: 0,
                audio_stream: 0,
                removed_counts: Default::default()
            }
        )
        .unwrap(),
        TranscriptItemOutcome::Created
    );
    assert_eq!(
        process_item(
            &config,
            &db,
            &cancel,
            &path.clone().into(),
            3,
            &mut input,
            &mut output,
            &tx,
            &ctx
        )
        .unwrap(),
        TranscriptItemOutcome::Failed
    );
    assert_eq!(
        db.get(&path.to_string_lossy()).unwrap().unwrap().status,
        TranscriptStatus::Ok
    );
}

#[test]
fn saver_discards_renamed_source_and_cancelled_result() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let db = TranscriptDb::open_at(&config.transcript_db_path).unwrap();
    let path = dir.path().join("source.wav");
    wav(&path);
    let signature = FileSignature::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    std::fs::rename(&path, dir.path().join("renamed.wav")).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    assert_eq!(
        save(
            &config,
            &db,
            &cancel,
            &path,
            signature,
            1,
            TranscriptResult::Failed
        )
        .unwrap(),
        TranscriptItemOutcome::Discarded
    );
    cancel.store(true, Ordering::Release);
    assert_eq!(
        save(
            &config,
            &db,
            &cancel,
            &path,
            signature,
            2,
            TranscriptResult::Failed
        )
        .unwrap(),
        TranscriptItemOutcome::Cancelled
    );
    assert!(db.get(&path.to_string_lossy()).unwrap().is_none());
}

#[test]
fn worker_finishes_and_keeps_committed_item_notification() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.wav");
    wav(&path);
    let worker = spawn_transcript_worker(
        config(dir.path()),
        Arc::new(AtomicBool::new(false)),
        egui::Context::default(),
    )
    .unwrap();
    worker
        .commands
        .send(TranscriptWorkerCommand::Item(path.clone().into()))
        .unwrap();
    loop {
        match worker.events.recv_timeout(Duration::from_secs(30)).unwrap() {
            TranscriptWorkerEvent::ItemDone(TranscriptItemOutcome::Created) => break,
            TranscriptWorkerEvent::Finished(result) => panic!("unexpected finish: {result:?}"),
            _ => {}
        }
    }
    worker.stop();
    loop {
        if let TranscriptWorkerEvent::Finished(result) =
            worker.events.recv_timeout(Duration::from_secs(30)).unwrap()
        {
            assert!(result.is_ok());
            break;
        }
    }
    assert_eq!(
        TranscriptDb::open_at(dir.path().join("transcript.db"))
            .unwrap()
            .get(&path.to_string_lossy())
            .unwrap()
            .unwrap()
            .status,
        TranscriptStatus::Ok
    );
}

#[test]
fn accepted_track_snapshot_is_resolved_and_saved_before_skip_decision() {
    let dir = tempfile::tempdir().unwrap();
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ai/transcript_audio/delayed_tracks.mka");
    let config = config(dir.path());
    let db = TranscriptDb::open_at(&config.transcript_db_path).unwrap();
    let (mut input, mut output) = pipes(dir.path());
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, _rx) = mpsc::channel();
    let ctx = egui::Context::default();
    let mut target = TranscriptTarget {
        path: path.clone(),
        saved_audio_track: Some(crate::video::SavedAudioTrackChoice {
            stream_index: 1,
            codec: "flac".into(),
            language: None,
            channels: Some(1),
            title: Some("delayed".into()),
        }),
    };
    assert_eq!(
        process_item(
            &config,
            &db,
            &cancel,
            &target,
            1,
            &mut input,
            &mut output,
            &tx,
            &ctx
        )
        .unwrap(),
        TranscriptItemOutcome::Created
    );
    assert_eq!(
        db.get(&path.to_string_lossy())
            .unwrap()
            .unwrap()
            .audio_stream,
        Some(1)
    );
    assert_eq!(
        process_item(
            &config,
            &db,
            &cancel,
            &target,
            2,
            &mut input,
            &mut output,
            &tx,
            &ctx
        )
        .unwrap(),
        TranscriptItemOutcome::AlreadyCreated
    );
    // 同じ署名でも、次の受付の選択が既定トラックへ変われば古い行はskipしない。
    target.saved_audio_track = None;
    assert_eq!(
        process_item(
            &config,
            &db,
            &cancel,
            &target,
            3,
            &mut input,
            &mut output,
            &tx,
            &ctx
        )
        .unwrap(),
        TranscriptItemOutcome::Created
    );
    assert_eq!(
        db.get(&path.to_string_lossy())
            .unwrap()
            .unwrap()
            .audio_stream,
        Some(0)
    );
}

#[test]
fn video_without_audio_is_counted_without_saving_any_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("video.y4m");
    let mut bytes = b"YUV4MPEG2 W2 H2 F1:1 Ip A1:1 C420jpeg\nFRAME\n".to_vec();
    bytes.extend_from_slice(&[16, 16, 16, 16, 128, 128]);
    std::fs::write(&path, bytes).unwrap();
    let config = config(dir.path());
    let db = TranscriptDb::open_at(&config.transcript_db_path).unwrap();
    let (mut input, mut output) = pipes(dir.path());
    let (tx, _rx) = mpsc::channel();
    assert_eq!(
        process_item(
            &config,
            &db,
            &Arc::new(AtomicBool::new(false)),
            &path.clone().into(),
            1,
            &mut input,
            &mut output,
            &tx,
            &egui::Context::default()
        )
        .unwrap(),
        TranscriptItemOutcome::NoAudio
    );
    assert!(db.get(&path.to_string_lossy()).unwrap().is_none());
}

/// 明示した評価素材だけを読む開発用測定。製品Appやモデル・通常settingsを起動しない。
#[test]
#[ignore = "MIV_ASR_EXTRACT_BENCH_MEDIAで指定した長い素材の抽出だけを手動測定する"]
fn transcript_extract_long_media_benchmark() {
    let path = std::env::var_os("MIV_ASR_EXTRACT_BENCH_MEDIA")
        .map(PathBuf::from)
        .expect("MIV_ASR_EXTRACT_BENCH_MEDIA is required");
    let cancel = Arc::new(AtomicBool::new(false));
    let total_started = Instant::now();
    let prepared = transcript_audio::prepare_with_saved(&path, None, cancel.clone())
        .unwrap()
        .expect("audio track");
    let stream_index = prepared.stream_index;
    let prepare_s = total_started.elapsed().as_secs_f64();
    let stats = prepared.extract(&cancel, |_| Ok(())).unwrap();
    let total_s = total_started.elapsed().as_secs_f64();
    assert!(stats.pcm_samples > 0);
    let audio_s = stats.audio_duration_ms as f64 / 1000.0;
    println!(
        "TRANSCRIPT_EXTRACT_BENCH {}",
        serde_json::json!({
            "audio_stream":stream_index,"audio_duration_s":audio_s,"pcm_samples":stats.pcm_samples,
            "extract_s":stats.elapsed.as_secs_f64(),"realtime_multiple":audio_s/stats.elapsed.as_secs_f64(),
            "prepare_s":prepare_s,"total_s":total_s,
            "total_wall_audio_ratio":total_s/audio_s,"total_realtime_multiple":audio_s/total_s,
            "wall_audio_ratio":stats.elapsed.as_secs_f64()/audio_s,
        })
    );
}
