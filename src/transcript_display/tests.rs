use super::*;
use crate::transcript_db::FileSignature;
use crate::transcript_display_cache::{
    TranscriptDisplayCache, TranscriptDisplayRow, TranscriptDisplayValue,
};

fn current_config_version() -> i64 {
    #[cfg(windows)]
    {
        crate::ai::transcript_job_worker::CONFIG_VERSION
    }
    #[cfg(not(windows))]
    {
        1
    }
}

fn value() -> TranscriptDisplayValue {
    let signature = FileSignature {
        size: 100,
        mtime_ms: 200,
    };
    TranscriptDisplayValue {
        row: Some(TranscriptDisplayRow {
            signature,
            audio_stream: Some(1),
            model_id: "whisper@fixture".into(),
            config_version: current_config_version(),
            status: TranscriptStatus::Ok,
            created_at: 1000,
            segments: Arc::new(vec![
                (3000, 5000, "最初の言葉".into()),
                (5000, 7000, "次の言葉".into()),
            ]),
        }),
        current_signature: Some(signature),
        read_failed: false,
    }
}

fn snapshot(
    value: Option<&TranscriptDisplayValue>,
    activity: TranscriptActivity,
    applied: Option<usize>,
) -> TranscriptSnapshot {
    build_transcript_snapshot("source-a".into(), value, activity, applied)
}

#[test]
fn snapshot_distinguishes_missing_loading_failed_stale_and_available() {
    let available = value();
    let missing = TranscriptDisplayValue::default();
    let read_error = TranscriptDisplayValue {
        read_failed: true,
        ..available.clone()
    };
    let mut saved_failure = available.clone();
    saved_failure.row.as_mut().unwrap().status = TranscriptStatus::Failed;
    let mut stale = available.clone();
    stale.current_signature.as_mut().unwrap().size += 1;
    for (value, expected) in [
        (None, TranscriptDataState::Loading),
        (Some(&missing), TranscriptDataState::Missing),
        (Some(&read_error), TranscriptDataState::Failed),
        (Some(&saved_failure), TranscriptDataState::Failed),
        (Some(&stale), TranscriptDataState::Stale),
        (Some(&available), TranscriptDataState::Available),
    ] {
        let snapshot = snapshot(value, TranscriptActivity::None, Some(1));
        assert_eq!(snapshot.key, "source-a");
        assert_eq!(snapshot.state, expected);
        assert_eq!(
            snapshot.segments.is_empty(),
            !matches!(
                expected,
                TranscriptDataState::Available | TranscriptDataState::Stale
            )
        );
    }
}

#[test]
fn snapshot_staleness_compares_both_signature_fields_but_missing_stat_is_not_stale() {
    let current = value();
    for field in ["size", "mtime"] {
        let mut changed = current.clone();
        let signature = changed.current_signature.as_mut().unwrap();
        if field == "size" {
            signature.size += 1;
        } else {
            signature.mtime_ms += 1;
        }
        let stale = snapshot(Some(&changed), TranscriptActivity::None, Some(1));
        assert_eq!(stale.state, TranscriptDataState::Stale, "{field}");
        assert!(Arc::ptr_eq(
            &stale.segments,
            &current.row.as_ref().unwrap().segments
        ));
    }
    let mut disappeared = current;
    disappeared.current_signature = None;
    assert_eq!(
        snapshot(Some(&disappeared), TranscriptActivity::None, Some(1)).state,
        TranscriptDataState::Available
    );
}

#[test]
fn shared_staleness_rule_matches_ui_snapshot_for_worker_inputs() {
    use crate::transcript_display_cache::transcript_is_stale;
    let saved = value();
    let row = saved.row.as_ref().unwrap();
    let mut changed_size = row.signature;
    changed_size.size += 1;
    let mut changed_mtime = row.signature;
    changed_mtime.mtime_ms += 1;
    for signature in [
        None,
        Some(row.signature),
        Some(changed_size),
        Some(changed_mtime),
    ] {
        for version in [current_config_version(), current_config_version() + 1] {
            let expected = signature.is_some_and(|signature| signature != row.signature)
                || (cfg!(windows) && version != current_config_version());
            assert_eq!(
                transcript_is_stale(row.signature, signature, version),
                expected
            );
            let mut input = saved.clone();
            input.current_signature = signature;
            input.row.as_mut().unwrap().config_version = version;
            assert_eq!(input.is_stale(), expected);
            assert_eq!(
                snapshot(Some(&input), TranscriptActivity::None, Some(1)).state,
                if expected {
                    TranscriptDataState::Stale
                } else {
                    TranscriptDataState::Available
                }
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn snapshot_config_version_change_is_stale_without_discarding_saved_text() {
    let mut changed = value();
    changed.row.as_mut().unwrap().config_version = current_config_version() + 1;
    let stale = snapshot(Some(&changed), TranscriptActivity::None, Some(1));
    assert_eq!(stale.state, TranscriptDataState::Stale);
    assert!(Arc::ptr_eq(
        &stale.segments,
        &changed.row.as_ref().unwrap().segments
    ));
}

#[test]
fn snapshot_queued_and_running_preserve_saved_row_and_arc() {
    let saved = value();
    let segments = &saved.row.as_ref().unwrap().segments;
    let base = snapshot(Some(&saved), TranscriptActivity::None, Some(1));
    for (activity, summary, notice) in [
        (
            TranscriptActivity::Queued,
            "文字起こし: 作成待ち",
            "文字起こしの作成を待っています",
        ),
        (
            TranscriptActivity::Running,
            "文字起こし: 作成中",
            "文字起こしを作成しています",
        ),
    ] {
        let active = snapshot(Some(&saved), activity, Some(1));
        assert_eq!(active.state, TranscriptDataState::Available);
        assert_eq!(active.activity, activity);
        assert_eq!(active.used_audio_stream, base.used_audio_stream);
        assert_eq!(active.created_at, base.created_at);
        assert!(Arc::ptr_eq(&active.segments, segments));
        assert_eq!(active.summary(), summary);
        assert_eq!(active.notice().as_deref(), Some(notice));
        assert!(!active.same_presentation(&base));
    }
}

#[test]
fn snapshot_different_track_comes_from_applied_track_and_recovers_when_it_returns() {
    let saved = value();
    for (applied, different) in [
        (Some(1), false),
        (Some(2), true),
        (Some(1), false),
        (None, true),
        (Some(1), false),
    ] {
        let snapshot = snapshot(Some(&saved), TranscriptActivity::None, applied);
        assert_eq!(snapshot.used_audio_stream, Some(1));
        assert_eq!(snapshot.applied_audio_stream, applied);
        assert_eq!(snapshot.different_track(), different);
        assert_eq!(snapshot.notice().is_some(), different);
        assert!(Arc::ptr_eq(
            &snapshot.segments,
            &saved.row.as_ref().unwrap().segments
        ));
    }
    let mut no_saved_track = saved.clone();
    no_saved_track.row.as_mut().unwrap().audio_stream = None;
    assert!(!snapshot(Some(&no_saved_track), TranscriptActivity::None, None).different_track());
    assert!(snapshot(Some(&no_saved_track), TranscriptActivity::None, Some(1)).different_track());
    // 未作成や失敗では、使ったトラックの比較を案内に混ぜない。
    let failed = TranscriptDisplayValue {
        read_failed: true,
        ..saved
    };
    assert!(!snapshot(Some(&failed), TranscriptActivity::None, Some(2)).different_track());
    assert!(!snapshot(None, TranscriptActivity::None, Some(2)).different_track());
}

#[test]
fn snapshot_switching_source_during_load_keeps_new_key_and_empty_text() {
    let saved = value();
    let loaded_a = snapshot(Some(&saved), TranscriptActivity::None, Some(1));
    let loading_a = snapshot(None, TranscriptActivity::Running, Some(1));
    let loading_b =
        build_transcript_snapshot("source-b".into(), None, TranscriptActivity::None, Some(2));
    assert_eq!(loaded_a.key, "source-a");
    assert!(!loaded_a.segments.is_empty());
    assert_eq!(loading_a.state, TranscriptDataState::Loading);
    assert_eq!(loading_b.key, "source-b");
    assert_eq!(loading_b.state, TranscriptDataState::Loading);
    assert_eq!(loading_b.activity, TranscriptActivity::None);
    assert!(loading_b.segments.is_empty());
    assert_eq!(loading_b.used_audio_stream, None);
    assert_eq!(loading_b.created_at, None);
    assert!(!loading_b.same_presentation(&loaded_a));
    assert!(!loading_b.same_presentation(&loading_a));
}

#[test]
fn snapshot_cache_clear_goes_to_loading_and_fresh_value_replaces_the_arc() {
    let mut cache = TranscriptDisplayCache::default();
    cache.seed("source-a", value());
    let old = snapshot(cache.get("source-a"), TranscriptActivity::None, Some(1));
    cache.clear();
    let loading = snapshot(cache.get("source-a"), TranscriptActivity::None, Some(1));
    assert_eq!(loading.state, TranscriptDataState::Loading);
    assert!(loading.segments.is_empty());
    assert!(!loading.same_presentation(&old));
    let mut fresh = value();
    fresh.row.as_mut().unwrap().segments = Arc::new(vec![(8000, 9000, "取り込み後の本文".into())]);
    cache.seed("source-a", fresh);
    let current = snapshot(cache.get("source-a"), TranscriptActivity::None, Some(1));
    assert_eq!(current.state, TranscriptDataState::Available);
    assert_eq!(current.segments[0].2, "取り込み後の本文");
    assert!(!Arc::ptr_eq(&current.segments, &old.segments));
    assert!(!current.same_presentation(&old));
}

#[test]
fn same_presentation_uses_arc_identity_instead_of_comparing_equal_nonempty_text() {
    let saved = value();
    let original = snapshot(Some(&saved), TranscriptActivity::None, Some(1));
    let shared = original.clone();
    assert!(original.same_presentation(&shared));
    let mut equal_content = original.clone();
    equal_content.segments = Arc::new(original.segments.as_ref().clone());
    assert_eq!(original.segments, equal_content.segments);
    assert!(!Arc::ptr_eq(&original.segments, &equal_content.segments));
    assert!(!original.same_presentation(&equal_content));
    // 空本文の Arc を各回作っても、不要な native 更新を出さない。
    let empty_a = snapshot(None, TranscriptActivity::None, Some(1));
    let empty_b = snapshot(None, TranscriptActivity::None, Some(1));
    assert!(!Arc::ptr_eq(&empty_a.segments, &empty_b.segments));
    assert!(empty_a.same_presentation(&empty_b));
}

#[test]
fn same_presentation_detects_metadata_changes_even_when_text_arc_is_unchanged() {
    let saved = value();
    let base = snapshot(Some(&saved), TranscriptActivity::None, Some(1));
    let mut key = base.clone();
    key.key = "source-b".into();
    let mut state = base.clone();
    state.state = TranscriptDataState::Stale;
    let mut activity = base.clone();
    activity.activity = TranscriptActivity::Running;
    let mut used = base.clone();
    used.used_audio_stream = Some(2);
    let mut applied = base.clone();
    applied.applied_audio_stream = Some(2);
    let mut date = base.clone();
    date.created_at = Some(2000);
    for changed in [key, state, activity, used, applied, date] {
        assert!(Arc::ptr_eq(&base.segments, &changed.segments));
        assert!(!base.same_presentation(&changed));
    }
}

#[test]
fn snapshot_summary_formats_a_fixed_local_date_and_state_notices() {
    let mut saved = value();
    saved.row.as_mut().unwrap().created_at = 1_763_121_600_000;
    let available = snapshot(Some(&saved), TranscriptActivity::None, Some(1));
    let local = crate::app::format_details_timestamp(1_763_121_600, false);
    assert_eq!(
        available.summary(),
        format!("文字起こし: あり ({})", local.split(' ').next().unwrap())
    );
    assert_eq!(available.notice(), None);
    assert_eq!(
        snapshot(Some(&saved), TranscriptActivity::None, Some(2)).summary(),
        "文字起こし: 別トラック"
    );
    let missing = snapshot(
        Some(&TranscriptDisplayValue::default()),
        TranscriptActivity::None,
        None,
    );
    assert_eq!(missing.summary(), "文字起こし: なし");
    assert!(missing.notice().unwrap().contains("一覧の右クリック"));
    let loading = snapshot(None, TranscriptActivity::None, None);
    assert_eq!(loading.summary(), "文字起こし: 読み込み中");
    assert!(loading.notice().unwrap().contains("読み込んでいます"));
    let failed = snapshot(
        Some(&TranscriptDisplayValue {
            read_failed: true,
            ..Default::default()
        }),
        TranscriptActivity::None,
        None,
    );
    assert_eq!(failed.summary(), "文字起こし: 失敗");
    assert!(failed.notice().unwrap().contains("表示できませんでした"));
    saved.current_signature.as_mut().unwrap().size += 1;
    let stale = snapshot(Some(&saved), TranscriptActivity::None, Some(1));
    assert_eq!(stale.summary(), "文字起こし: 古い");
    assert!(stale.notice().unwrap().contains("古くなっています"));
}

#[test]
fn snapshot_media_segments_are_independent_of_playback_metadata_arrival() {
    let saved = value();
    let before = snapshot(Some(&saved), TranscriptActivity::None, None);
    let after = snapshot(Some(&saved), TranscriptActivity::None, Some(1));
    assert_eq!(before.segments[0].0, 3000);
    assert_eq!(after.segments[0].0, 3000);
    assert!(Arc::ptr_eq(&before.segments, &after.segments));
    assert_eq!(before.state, TranscriptDataState::Available);
    assert!(!before.same_presentation(&after));
}
