use super::*;
use ffmpeg::format::sample::{Sample, Type};

fn wav(path: &Path, rate: u32, frames: u32) {
    let mut out = Vec::new();
    out.extend(b"RIFF");
    out.extend((36 + frames * 2).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * 2).to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend((frames * 2).to_le_bytes());
    for i in 0..frames {
        out.extend(((i % 800) as i16 * 20 - 8000).to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}

#[test]
fn extraction_resamples_and_streams_in_bounded_frames() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.wav");
    wav(&path, 48_000, 48_000 * 36);
    let cancel = Arc::new(AtomicBool::new(false));
    let prepared = prepare_with_saved(&path, None, cancel.clone())
        .unwrap()
        .unwrap();
    let mut sizes = Vec::new();
    let stats = prepared
        .extract(&cancel, |bytes| {
            sizes.push(bytes.len());
            Ok(())
        })
        .unwrap();
    assert_eq!(stats.pcm_samples, 16_000 * 36);
    assert_eq!(stats.audio_duration_ms, 36_000);
    assert!(sizes.len() >= 2);
    assert!(
        sizes
            .iter()
            .all(|len| *len > 0 && *len <= MAX_PCM_BYTES && len % 2 == 0)
    );
    assert_eq!(sizes[0], MAX_PCM_BYTES);
}

#[test]
fn delayed_audio_is_zero_filled_on_playback_timeline() {
    crate::audio_decode::ensure_ffmpeg_init();
    let cancel = AtomicBool::new(false);
    let mut pcm = Vec::new();
    let mut sink = PcmSink {
        callback: |bytes: &[u8]| {
            pcm.extend_from_slice(bytes);
            Ok(())
        },
        cancel: &cancel,
        bytes: Vec::with_capacity(MAX_PCM_BYTES),
        samples: 0,
    };
    let mut frame = Audio::new(
        Sample::I16(Type::Packed),
        16_000,
        ffmpeg::ChannelLayoutMask::MONO,
    );
    frame.set_rate(16_000);
    frame.set_pts(Some(3 * 16_000));
    frame.plane_mut::<i16>(0).fill(1200);
    let mut graph = make_graph(&frame, ffmpeg::Rational(1, 16_000)).unwrap();
    graph.get("in").unwrap().source().add(&frame).unwrap();
    pull_pcm(&mut graph, &mut sink).unwrap();
    graph.get("in").unwrap().source().flush().unwrap();
    pull_pcm(&mut graph, &mut sink).unwrap();
    sink.flush().unwrap();
    assert_eq!(sink.samples, 4 * 16_000);
    drop(sink);
    assert!(pcm[..3 * 16_000 * 2].iter().all(|&b| b == 0));
    assert!(
        pcm[3 * 16_000 * 2..]
            .chunks_exact(2)
            .all(|b| i16::from_le_bytes([b[0], b[1]]) == 1200)
    );
}

#[test]
fn cancelled_sink_stops_extraction_without_more_callbacks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.wav");
    wav(&path, 16_000, 16_000 * 40);
    let cancel = Arc::new(AtomicBool::new(false));
    let prepared = prepare_with_saved(&path, None, cancel.clone())
        .unwrap()
        .unwrap();
    let mut calls = 0;
    let result = prepared.extract(&cancel, |_| {
        calls += 1;
        cancel.store(true, Ordering::Release);
        Ok(())
    });
    assert!(matches!(result, Err(AudioError::Cancelled)));
    assert_eq!(calls, 1);
}

#[test]
fn sink_failure_is_returned_and_cancel_before_open_is_observed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.wav");
    wav(&path, 16_000, 1000);
    let cancel = Arc::new(AtomicBool::new(false));
    let prepared = prepare_with_saved(&path, None, cancel.clone())
        .unwrap()
        .unwrap();
    assert!(matches!(
        prepared.extract(&cancel, |_| Err(AudioError::Sink("closed".into()))),
        Err(AudioError::Sink(_))
    ));
    cancel.store(true, Ordering::Release);
    assert!(matches!(
        prepare_with_saved(&path, None, cancel),
        Err(AudioError::Cancelled)
    ));
}

#[test]
fn prepared_audio_can_move_to_extraction_thread() {
    fn assert_send<T: Send>() {}
    assert_send::<PreparedAudio>();
}

#[test]
fn no_audio_input_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("video.y4m");
    let mut bytes = b"YUV4MPEG2 W2 H2 F1:1 Ip A1:1 C420jpeg\nFRAME\n".to_vec();
    bytes.extend_from_slice(&[16, 16, 16, 16, 128, 128]);
    std::fs::write(&path, bytes).unwrap();
    assert!(
        prepare_with_saved(&path, None, Arc::new(AtomicBool::new(false)))
            .unwrap()
            .is_none()
    );
}

#[test]
fn saved_second_track_decodes_at_its_playback_timestamp() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ai/transcript_audio/delayed_tracks.mka");
    let saved = SavedAudioTrackChoice {
        stream_index: 1,
        codec: "flac".into(),
        language: None,
        channels: Some(1),
        title: Some("delayed".into()),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let default = prepare_with_saved(&path, None, cancel.clone())
        .unwrap()
        .unwrap();
    assert_eq!(default.stream_index, 0);
    let prepared = prepare_with_saved(&path, Some(&saved), cancel.clone())
        .unwrap()
        .unwrap();
    assert_eq!(prepared.stream_index, 1);
    assert!((prepared.duration_secs - 4.0).abs() < 0.001);
    let mut bytes = Vec::new();
    let stats = prepared
        .extract(&cancel, |chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
    assert_eq!(stats.pcm_samples, 4 * 16_000);
    assert!(bytes[..3 * 16_000 * 2].iter().all(|&b| b == 0));
    assert!(bytes[3 * 16_000 * 2..].iter().any(|&b| b != 0));
}

#[test]
fn nonzero_container_origin_matches_the_zero_origin_pcm() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ai/transcript_audio");
    let saved = SavedAudioTrackChoice {
        stream_index: 1,
        codec: "flac".into(),
        language: None,
        channels: Some(1),
        title: Some("delayed".into()),
    };
    let mut outputs = Vec::new();
    for name in ["delayed_tracks.mka", "offset_tracks.mka"] {
        let cancel = Arc::new(AtomicBool::new(false));
        let prepared = prepare_with_saved(&root.join(name), Some(&saved), cancel.clone())
            .unwrap()
            .unwrap();
        assert_eq!(prepared.stream_index, 1);
        let mut bytes = Vec::new();
        let stats = prepared
            .extract(&cancel, |chunk| {
                bytes.extend_from_slice(chunk);
                Ok(())
            })
            .unwrap();
        assert_eq!(stats.pcm_samples, 4 * 16_000, "{name}");
        outputs.push(bytes);
    }
    assert_eq!(outputs[0], outputs[1]);
}
