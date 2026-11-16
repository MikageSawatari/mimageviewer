use super::*;
use crate::transcript_db::{FileSignature, TranscriptStatus};
use crate::transcript_display::TranscriptDataState;
use crate::transcript_display_cache::{TranscriptDisplayRow, TranscriptDisplayValue};
use std::sync::Arc;

fn value(text: &str) -> TranscriptDisplayValue {
    let signature = FileSignature {
        size: 8,
        mtime_ms: 1,
    };
    TranscriptDisplayValue {
        row: Some(TranscriptDisplayRow {
            signature,
            audio_stream: Some(1),
            model_id: "fixture".into(),
            config_version: 1,
            status: TranscriptStatus::Ok,
            created_at: 1_762_862_400_000,
            segments: Arc::new(vec![(3000, 5000, text.into())]),
        }),
        current_signature: Some(signature),
        read_failed: false,
    }
}

#[cfg(windows)]
fn player(app: &mut App, idx: usize, path: std::path::PathBuf) {
    app.fs_cache.insert(
        idx,
        FsCacheEntry::Video {
            player: Box::new(crate::video::VideoPlayer::stream_ready_disconnected_for_test(path)),
            load_seq: 0,
        },
    );
}

#[test]
#[cfg(windows)]
fn transcript_snapshot_switch_and_old_key_arrival_never_publish_to_current_player() {
    let mut app = crate::app::setup_app_for_test();
    let first = app.tmp.path().join("first.mp4");
    let second = app.tmp.path().join("second.mp4");
    let first_key = crate::path_key::normalize_keep_drive(&first);
    let second_key = crate::path_key::normalize_keep_drive(&second);
    player(&mut app, 0, first);
    player(&mut app, 1, second);
    let ctx = egui::Context::default();
    app.sync_native_transcript_snapshot(&ctx, 0);
    app.fullscreen_idx = Some(1);
    app.transcript_display.seed(&first_key, value("前の動画"));
    app.sync_native_transcript_snapshot(&ctx, 1);
    let FsCacheEntry::Video { player, .. } = app.fs_cache.get(&1).unwrap() else {
        panic!()
    };
    let snapshot = player.native_transcript_snapshot_for_test().unwrap();
    assert_eq!(snapshot.key, second_key);
    assert_eq!(snapshot.state, TranscriptDataState::Loading);
    assert!(snapshot.segments.is_empty());
}

#[test]
#[cfg(windows)]
fn transcript_committed_store_mutations_clear_and_republish_loading_then_new_text() {
    let mut app = crate::app::setup_app_for_test();
    let path = app.tmp.path().join("source.mp4");
    let key = crate::path_key::normalize_keep_drive(&path);
    player(&mut app, 0, path);
    let ctx = egui::Context::default();
    let descriptor = crate::rename_key_migration::STORES
        .iter()
        .find(|store| store.file == "transcript.db")
        .unwrap();
    for label in ["保存", "削除", "取り込み", "STORES改名・復元"] {
        let old = value(label);
        let old_segments = Arc::clone(&old.row.as_ref().unwrap().segments);
        app.transcript_display.seed(&key, old);
        app.sync_native_transcript_snapshot(&ctx, 0);
        let mut effects = crate::rename_key_migration::StoreMutationEffects::default();
        effects.record_rows_changed(descriptor, 1);
        app.apply_content_identity_store_mutations(effects);
        assert!(!app.transcript_display.contains(&key));
        app.sync_native_transcript_snapshot(&ctx, 0);
        let FsCacheEntry::Video { player, .. } = app.fs_cache.get(&0).unwrap() else {
            panic!()
        };
        let loading = player.native_transcript_snapshot_for_test().unwrap();
        assert_eq!(loading.state, TranscriptDataState::Loading);
        assert!(loading.segments.is_empty());
        app.transcript_display.seed(&key, value("新しい本文"));
        app.sync_native_transcript_snapshot(&ctx, 0);
        let FsCacheEntry::Video { player, .. } = app.fs_cache.get(&0).unwrap() else {
            panic!()
        };
        let loaded = player.native_transcript_snapshot_for_test().unwrap();
        assert_eq!(loaded.state, TranscriptDataState::Available);
        assert!(!Arc::ptr_eq(&loaded.segments, &old_segments));
        assert_eq!(loaded.segments[0].2, "新しい本文");
    }
}

#[test]
#[cfg(windows)]
fn transcript_panel_close_preserves_shared_cache_and_recreated_output_republishes() {
    let mut app = crate::app::setup_app_for_test();
    let path = app.tmp.path().join("source.mp4");
    let key = crate::path_key::normalize_keep_drive(&path);
    player(&mut app, 0, path);
    let ctx = egui::Context::default();
    let saved = value("そのまま残す本文");
    let segments = Arc::clone(&saved.row.as_ref().unwrap().segments);
    app.transcript_display.seed(&key, saved);
    app.sync_native_transcript_snapshot(&ctx, 0);
    app.close_fs_side_panel_runtime();
    assert!(app.transcript_display.contains(&key));
    app.reset_video_audio_side_panel_sessions(0);
    assert!(Arc::ptr_eq(
        &app.transcript_display
            .get(&key)
            .unwrap()
            .row
            .as_ref()
            .unwrap()
            .segments,
        &segments
    ));
    for _ in 0..2 {
        let FsCacheEntry::Video { player, .. } = app.fs_cache.get_mut(&0).unwrap() else {
            panic!()
        };
        let _old = player.take_native_output().unwrap();
        player.attach_native_output(crate::video::NativeVideoOutput::disconnected_for_test());
        assert!(player.native_transcript_snapshot_for_test().is_none());
        app.sync_native_transcript_snapshot(&ctx, 0);
        let FsCacheEntry::Video { player, .. } = app.fs_cache.get(&0).unwrap() else {
            panic!()
        };
        assert!(Arc::ptr_eq(
            &player
                .native_transcript_snapshot_for_test()
                .unwrap()
                .segments,
            &segments
        ));
    }
}
