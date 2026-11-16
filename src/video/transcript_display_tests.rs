use super::*;
use crate::transcript_display::{TranscriptDataState, TranscriptSnapshot};

#[test]
fn transcript_saved_media_time_is_used_before_metadata_and_across_track_changes() {
    for stream in [Some(1), Some(2), None] {
        let mut player = VideoPlayer::stream_ready_disconnected_for_test("offset.mka".into());
        if let Some(stream) = stream {
            player.info.as_mut().unwrap().opened_audio_stream_index = Some(stream);
        } else {
            player.info = None;
        }
        let saved_ms = 3000;
        player.seek(saved_ms as f64 / 1000.0, SeekOrigin::User);
        assert_eq!(player.clock.take_seek_request().unwrap().target_secs, 3.0);
        assert_eq!(player.position(), 3.0);
        assert_eq!(player.clamp_seek_target(-7.0), 0.0);
    }
}

#[test]
fn transcript_nonzero_ts_origin_uses_saved_media_seconds_and_normal_player_clamp() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/timeline-origin/transcript-origin-video.ts");
    let info = decoder::audio_track_fixture_tests::open_fixture_info_path(path.clone());
    assert!((info.timeline_origin_secs - 1.578667).abs() < 0.000001);
    assert!((info.duration_secs - 4.021333).abs() < 0.000001);
    let duration_secs = info.duration_secs;
    // 各行の初回seekを独立に検査する。連続入力のcoalescingはplayerの既存契約のまま。
    for (saved_secs, expected) in [
        (3000_u64 as f64 / 1000.0, 3.0),
        (1000.0, duration_secs - 0.1),
    ] {
        let mut player = VideoPlayer::stream_ready_disconnected_for_test(path.clone());
        player.info = Some(info.clone());
        player.seek(saved_secs, SeekOrigin::User);
        let issued = player.clock.take_seek_request().unwrap();
        assert_eq!(issued.target_secs, player.clamp_seek_target(saved_secs));
        assert!((issued.target_secs - expected).abs() < 0.000001);
        assert!((player.position() - expected).abs() < 0.000001);
    }
}

#[test]
fn transcript_latest_command_keeps_barrier_order_and_deduplicates_unchanged_arc() {
    let mut output = NativeVideoOutput::disconnected_for_test();
    let (tx, rx) = native_command_bus(8, Arc::new(AtomicBool::new(false)));
    output.command_tx = tx;
    let first = TranscriptSnapshot {
        key: "a".into(),
        state: TranscriptDataState::Available,
        segments: Arc::new(vec![(0, 1000, "本文".into())]),
        ..Default::default()
    };
    output.set_transcript_snapshot(first.clone());
    output.set_transcript_snapshot(first.clone());
    assert_eq!(rx.drain().len(), 1);
    output.set_transcript_snapshot(first.clone());
    assert!(rx.drain().is_empty());
    output
        .command_tx
        .send(NativeVideoOutputCommand::ResetSidePanelSession)
        .unwrap();
    let loading = TranscriptSnapshot {
        key: "b".into(),
        state: TranscriptDataState::Loading,
        ..Default::default()
    };
    output.set_transcript_snapshot(loading.clone());
    let commands = rx.drain();
    assert!(matches!(
        commands.first(),
        Some(NativeVideoOutputCommand::ResetSidePanelSession)
    ));
    assert!(
        matches!(commands.get(1),Some(NativeVideoOutputCommand::SetTranscriptSnapshot {snapshot}) if snapshot.same_presentation(&loading))
    );
    assert_eq!(commands.len(), 2);
}
