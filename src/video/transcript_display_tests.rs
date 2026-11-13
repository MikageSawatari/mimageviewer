use super::*;
use crate::transcript_display::{TranscriptDataState, TranscriptSnapshot};

#[test]
fn transcript_origin_is_applied_once_before_existing_seek_and_inverse_highlight() {
    let mut player = VideoPlayer::stream_ready_disconnected_for_test("offset.mka".into());
    player.info.as_mut().unwrap().container_start_secs = 10.0;
    let mapping = player.transcript_time_mapping();
    let target = mapping.to_playback(3.0).unwrap();
    player.seek(target, SeekOrigin::User);
    assert_eq!(player.clock.take_seek_request().unwrap().target_secs, 13.0);
    player.set_position_for_test(13.0);
    assert_eq!(mapping.to_transcript(player.position()), Some(3.0));
    player.info.as_mut().unwrap().opened_audio_stream_index = Some(2);
    assert_eq!(player.transcript_time_mapping(), mapping);
    assert_eq!(
        player.transcript_time_mapping().to_playback(3.0),
        Some(13.0)
    );
    // player の既存範囲制限はこの機能で拡張しない。helperの往復とは区別する。
    assert_eq!(player.clamp_seek_target(-7.0), 0.0);
    assert_eq!(player.clamp_seek_target(1000.0), 29.9);
}

#[test]
fn transcript_nonzero_ts_origin_uses_existing_player_seek_clamp_without_bypass() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/video/transcript_time/fixtures/origin-video.ts");
    // 同梱 decoder が実際の TS を開いて公表した metadata を使う。
    // 再生器のテスト用 transport は窓・音声 device・製品 exe を起動しない。
    let info = decoder::audio_track_fixture_tests::open_fixture_info_path(path.clone());
    assert!((info.container_start_secs - 1.578667).abs() < 0.000001);
    assert!((info.duration_secs - 4.021333).abs() < 0.000001);
    let duration_secs = info.duration_secs;
    let mut player = VideoPlayer::stream_ready_disconnected_for_test(path);
    player.info = Some(info);
    let mapping = player.transcript_time_mapping();
    let raw_target = mapping.to_playback(3.0).unwrap();
    assert!((raw_target - 4.578667).abs() < 0.000001);
    assert!((mapping.to_transcript(raw_target).unwrap() - 3.0).abs() < 0.000001);

    player.seek(raw_target, SeekOrigin::User);
    let issued = player.clock.take_seek_request().unwrap();
    assert!((issued.target_secs - 3.921333).abs() < 0.000001);
    assert!((issued.target_secs - (duration_secs - 0.1)).abs() < 0.000001);
    assert_eq!(issued.target_secs, player.clamp_seek_target(raw_target));
    assert_eq!(player.info().unwrap().duration_secs, duration_secs);
    // 既存の seek target override も同じ clamp 済み秒を公開する。
    let transcript_position = mapping.to_transcript(player.position()).unwrap();
    assert!((transcript_position - 2.342666).abs() < 0.000001);
    assert!(transcript_position < 3.0);
    // T1 では専用の clamp や duration への原点加算を足さない。
    // この既存制約はシークバー・ブックマーク・チャプターと共通のまま残す。
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
