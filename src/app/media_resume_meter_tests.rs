//! 再生位置の表示はlive settingsと同じsourceの遅延メタだけを参照する。
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::tests::phase_c_support::{AppTestEnv, setup_app};
use super::{
    App, DetailsLazyMeta, DetailsMediaMeta, DetailsMetaPendingPhase, DetailsVideoProbe,
    LazyColumnState,
};
use crate::grid_item::GridItem;
use crate::settings::{GridViewMode, SelectionInfoDisplayMode};

fn media_app() -> AppTestEnv {
    let mut app = setup_app();
    app.settings.grid_view_mode = GridViewMode::Thumbnail;
    app.settings.thumb_show_resume_meter = true;
    app.settings.thumb_show_media_duration = false;
    app.settings.selection_info_display_mode = SelectionInfoDisplayMode::Hidden;
    let root = app.tmp.path().join("uncreated-media");
    app.install_new_items(
        vec![
            GridItem::Video(root.join("video.mp4")),
            GridItem::Audio(root.join("audio.flac")),
            GridItem::Image(root.join("image.jpg")),
            GridItem::PdfFile(root.join("book.pdf")),
            GridItem::Video(root.join("offscreen.mp4")),
        ],
        vec![Some((100, 2048)); 5],
    );
    app.selected = None;
    app
}

fn media_key(app: &App, idx: usize) -> String {
    match &app.items[idx] {
        GridItem::Video(path) | GridItem::Audio(path) => crate::adjustment_db::normalize_path(path),
        _ => panic!("media fixture expected"),
    }
}

fn duration_meta(duration: Option<f64>) -> DetailsLazyMeta {
    DetailsLazyMeta {
        source_mtime: 100,
        source_size: 2048,
        media: DetailsMediaMeta::Read(DetailsVideoProbe {
            duration_secs: duration,
            dims: None,
            codec: None,
        }),
        ..Default::default()
    }
}

fn seed_duration(app: &mut App, idx: usize, duration: Option<f64>) {
    let key = media_key(app, idx);
    app.details_lazy_meta.insert(key, duration_meta(duration));
}

#[test]
fn media_resume_meter_fraction_rejects_invalid_values_without_clamping() {
    use super::book_resume_meter::media_resume_fraction;

    for (position, duration) in [(1.0, 4.0), (3.0, 4.0), (4.0, 4.0)] {
        assert_eq!(
            media_resume_fraction(position, duration),
            Some((position / duration) as f32)
        );
    }
    for (position, duration) in [
        (0.0, 10.0),
        (-1.0, 10.0),
        (11.0, 10.0),
        (1.0, 0.0),
        (1.0, -1.0),
        (f64::NAN, 10.0),
        (f64::INFINITY, 10.0),
        (f64::NEG_INFINITY, 10.0),
        (1.0, f64::NAN),
        (1.0, f64::INFINITY),
        (1.0, f64::NEG_INFINITY),
        (f64::MIN_POSITIVE, f64::MAX),
    ] {
        assert_eq!(
            media_resume_fraction(position, duration),
            None,
            "{position}/{duration}"
        );
    }
}

#[test]
fn media_resume_meter_video_and_audio_use_live_positions_and_memory_duration() {
    let mut app = media_app();
    let video = media_key(&app, 0);
    let audio = media_key(&app, 1);
    seed_duration(&mut app, 0, Some(100.0));
    seed_duration(&mut app, 1, Some(200.0));
    app.settings
        .video_resume_positions
        .insert(video.clone(), 25.0);
    app.settings
        .video_resume_positions
        .insert(audio.clone(), 100.0);
    assert_eq!(app.thumbnail_resume_meter(0), Some(0.25));
    assert_eq!(app.thumbnail_resume_meter(1), Some(0.5));
    assert_eq!(
        app.thumbnail_media_duration_text(0),
        None,
        "badge OFFでもmeterは表示する"
    );
    assert_eq!(app.thumbnail_media_duration_text(1), None);

    app.settings
        .video_resume_positions
        .insert(video.clone(), 75.0);
    app.settings
        .video_resume_positions
        .insert(audio.clone(), 50.0);
    assert_eq!(app.thumbnail_resume_meter(0), Some(0.75));
    assert_eq!(app.thumbnail_resume_meter(1), Some(0.25));
    app.top_level_grid_view
        .replace_surface(super::top_level_grid_view::TopLevelGridSurface::Snapshot);
    assert_eq!(app.thumbnail_resume_meter(0), Some(0.75));
    assert_eq!(app.thumbnail_resume_meter(1), Some(0.25));
    app.settings.video_resume_positions.remove(&video);
    app.settings.video_resume_positions.remove(&audio);
    assert_eq!(app.thumbnail_resume_meter(0), None);
    assert_eq!(app.thumbnail_resume_meter(1), None);
    assert_eq!(app.details_lazy_meta.len(), 2);
    assert!(
        app.details_meta_pending.is_none(),
        "paint consumer cannot launch a probe"
    );
    for item in &app.items {
        if let GridItem::Video(path) | GridItem::Audio(path) = item {
            assert!(!path.exists(), "fixture never creates the source files");
        }
    }
}

#[test]
fn media_resume_meter_requires_saved_position_and_valid_current_duration() {
    let mut app = media_app();
    let key = media_key(&app, 0);
    assert_eq!(app.thumbnail_resume_meter(0), None);
    app.settings
        .video_resume_positions
        .insert(key.clone(), 10.0);
    assert_eq!(app.thumbnail_resume_meter(0), None, "duration missing");

    for state in [
        DetailsMediaMeta::NotFetched,
        DetailsMediaMeta::Unreadable,
        DetailsMediaMeta::RetryLater {
            generation: app.items_generation,
        },
    ] {
        let mut meta = duration_meta(Some(100.0));
        meta.media = state;
        app.details_lazy_meta.insert(key.clone(), meta);
        assert_eq!(app.thumbnail_resume_meter(0), None);
    }
    for duration in [
        None,
        Some(0.0),
        Some(-1.0),
        Some(f64::NAN),
        Some(f64::INFINITY),
    ] {
        seed_duration(&mut app, 0, duration);
        assert_eq!(app.thumbnail_resume_meter(0), None, "duration={duration:?}");
    }
    seed_duration(&mut app, 0, Some(100.0));
    for position in [0.0, -1.0, 101.0, f64::NAN, f64::INFINITY] {
        app.settings
            .video_resume_positions
            .insert(key.clone(), position);
        assert_eq!(app.thumbnail_resume_meter(0), None, "position={position}");
    }
    app.settings
        .video_resume_positions
        .insert(key.clone(), 10.0);
    app.image_metas[0] = Some((101, 2048));
    assert_eq!(app.thumbnail_resume_meter(0), None, "stale mtime");
    app.image_metas[0] = Some((100, 2049));
    assert_eq!(app.thumbnail_resume_meter(0), None, "stale size");
    app.image_metas[0] = Some((100, 2048));
    assert_eq!(app.thumbnail_resume_meter(0), Some(0.1));
    app.settings.video_resume_positions.remove(&key);
    assert_eq!(app.thumbnail_resume_meter(0), None, "no saved row");
}

#[test]
fn media_resume_meter_shared_setting_hides_meter_and_preserves_duration_badge() {
    let mut app = media_app();
    let key = media_key(&app, 0);
    seed_duration(&mut app, 0, Some(100.0));
    app.settings
        .video_resume_positions
        .insert(key.clone(), 25.0);
    app.settings.thumb_show_media_duration = true;
    app.settings.thumb_show_resume_meter = false;
    assert!(
        app.thumbnail_media_metadata_enabled(),
        "badge alone still requests duration"
    );
    assert_eq!(app.thumbnail_resume_meter(0), None);
    assert_eq!(
        app.thumbnail_media_duration_text(0).as_deref(),
        Some("1:40")
    );
    app.settings.thumb_show_resume_meter = true;
    assert_eq!(app.thumbnail_resume_meter(0), Some(0.25));
    assert_eq!(app.settings.video_resume_positions.get(&key), Some(&25.0));
}

#[test]
fn media_resume_meter_preserves_drive_keys_and_replaced_index_identity() {
    let mut app = media_app();
    app.install_new_items(
        vec![
            GridItem::Video(PathBuf::from(r"C:\same\clip.mp4")),
            GridItem::Audio(PathBuf::from(r"D:\same\clip.mp4")),
        ],
        vec![Some((100, 2048)); 2],
    );
    let first = media_key(&app, 0);
    let second = media_key(&app, 1);
    assert_ne!(first, second, "media keeps drive identity");
    for idx in [0, 1] {
        seed_duration(&mut app, idx, Some(100.0));
    }
    app.settings
        .video_resume_positions
        .insert(first.clone(), 25.0);
    app.settings
        .video_resume_positions
        .insert(second.clone(), 75.0);
    assert_eq!(app.thumbnail_resume_meter(0), Some(0.25));
    assert_eq!(app.thumbnail_resume_meter(1), Some(0.75));
    app.settings.video_watched_to_end.insert(first);
    assert_eq!(app.thumbnail_resume_meter(0), Some(1.0));
    assert_eq!(app.thumbnail_resume_meter(1), Some(0.75));

    app.install_new_items(
        vec![GridItem::Audio(PathBuf::from(r"D:\same\clip.mp4"))],
        vec![Some((100, 2048))],
    );
    assert_eq!(
        app.thumbnail_resume_meter(0),
        Some(0.75),
        "same idx must use replacement path"
    );
    assert_eq!(app.thumbnail_resume_meter(1), None);
    app.settings.video_watched_to_end.insert(second.clone());
    assert_eq!(app.thumbnail_resume_meter(0), Some(1.0));
    app.settings.video_watched_to_end.remove(&second);
    assert_eq!(app.thumbnail_resume_meter(0), Some(0.75));
}

#[test]
fn media_resume_meter_badge_off_requests_only_visible_and_near_media() {
    let mut app = media_app();
    let near = HashSet::from([0, 1, 2, 3]);
    assert!(app.details_lazy_columns_visible());
    assert!(!app.selection_info_only_lazy_load());
    assert!(app.details_lazy_uses_visible_stages());
    for idx in 0..app.items.len() {
        let target = app.details_meta_target_for_idx(idx, &near, true);
        if idx < 2 {
            let target = target.expect("meter demands near media duration with badge OFF");
            assert!(target.load_video_meta);
            assert!(!target.load_page_count);
            assert!(!target.load_image_dims);
            assert!(!target.load_created_at);
        } else {
            assert!(target.is_none(), "nonmedia and offscreen media stay lazy");
        }
    }

    app.keep_set = near;
    app.details_tag_prewarm_indices = vec![0, 1, 2, 3];
    app.start_details_meta_load(&egui::Context::default());
    let pending = app
        .details_meta_pending
        .as_ref()
        .expect("production stage worker");
    assert!(matches!(
        pending.phase,
        DetailsMetaPendingPhase::Loading { .. }
    ));
    assert_eq!(
        pending.normal_target_keys,
        (0..2)
            .filter_map(|idx| app.details_lazy_cache_key(idx))
            .collect::<HashSet<_>>()
    );
    assert_eq!(
        app.details_image_dims_state,
        LazyColumnState::Loading { done: 0, total: 2 }
    );
    app.cancel_details_meta_loading();
}

#[test]
fn media_resume_meter_both_settings_off_do_not_start_metadata_loading() {
    let mut app = media_app();
    app.settings.thumb_show_resume_meter = false;
    app.keep_set = HashSet::from([0, 1]);
    app.details_tag_prewarm_indices = vec![0, 1];
    assert!(!app.details_lazy_columns_visible());
    for idx in 0..app.items.len() {
        assert!(
            app.details_meta_target_for_idx(idx, &app.keep_set, true)
                .is_none()
        );
    }
    app.start_details_meta_load(&egui::Context::default());
    assert!(app.details_meta_pending.is_none());
}

#[test]
fn media_resume_meter_stage_replacement_preserves_idle_and_generation_cancellation() {
    let mut app = media_app();
    app.keep_set = HashSet::from([0, 1]);
    app.details_tag_prewarm_indices = vec![0, 1];
    let ctx = egui::Context::default();
    app.start_details_meta_load(&ctx);
    let cancel = Arc::clone(&app.details_meta_pending.as_ref().unwrap().cancel);
    app.keep_set = HashSet::from([4]);
    app.last_prefetch_scroll_at = Some(std::time::Instant::now());
    app.refresh_thumbnail_details_stage(&ctx);
    assert!(!cancel.load(Ordering::Relaxed));
    assert_eq!(app.details_tag_prewarm_indices, vec![0, 1]);
    app.last_prefetch_scroll_at = Some(std::time::Instant::now() - super::PREFETCH_IDLE_THRESHOLD);
    app.refresh_thumbnail_details_stage(&ctx);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.details_meta_pending.is_none());
    assert_eq!(app.details_tag_prewarm_indices, vec![4]);

    app.start_details_meta_load(&ctx);
    let pending = app.details_meta_pending.as_ref().unwrap();
    let replacement_cancel = Arc::clone(&pending.cancel);
    let generation = app.items_generation;
    app.install_new_items(
        vec![GridItem::Audio(PathBuf::from(
            r"C:\uncreated\replacement.flac",
        ))],
        vec![Some((200, 4096))],
    );
    assert!(replacement_cancel.load(Ordering::Relaxed));
    assert!(app.details_meta_pending.is_none());
    assert_ne!(app.items_generation, generation);
}

#[test]
fn media_resume_meter_watched_transitions_preserve_short_replay_and_clear_on_midpoint() {
    let key = crate::adjustment_db::normalize_path(std::path::Path::new(r"C:\clips\watched.mp4"));
    let mut positions = std::collections::HashMap::from([(key.clone(), 13.046)]);
    let mut watched = HashSet::new();

    for (position, duration, at_eof, saved, completed) in [
        (13.046, 0.0, true, false, true),
        (2.999, 100.0, false, false, true),
        (95.0, 100.0, false, false, true),
        (3.0, 100.0, false, true, false),
        (2.999, 100.0, false, false, false),
    ] {
        assert_eq!(
            super::save_video_resume_position(
                &mut positions,
                &mut watched,
                key.clone(),
                position,
                duration,
                at_eof,
            ),
            saved
        );
        assert_eq!(watched.contains(&key), completed);
        assert_eq!(positions.get(&key).copied(), saved.then_some(position));
    }
}

#[test]
fn media_resume_meter_watched_video_and_audio_are_full_without_duration_and_live() {
    let mut app = media_app();
    for idx in [0, 1] {
        let key = media_key(&app, idx);
        app.settings.video_watched_to_end.insert(key.clone());
        assert_eq!(
            app.thumbnail_resume_meter(idx),
            Some(1.0),
            "no duration row"
        );

        for state in [DetailsMediaMeta::NotFetched, DetailsMediaMeta::Unreadable] {
            let mut meta = duration_meta(None);
            meta.media = state;
            app.details_lazy_meta.insert(key.clone(), meta);
            assert_eq!(app.thumbnail_resume_meter(idx), Some(1.0));
        }
        seed_duration(&mut app, idx, Some(100.0));
        app.image_metas[idx] = Some((101, 4096));
        assert_eq!(app.thumbnail_resume_meter(idx), Some(1.0), "stale duration");
        app.settings.thumb_show_resume_meter = false;
        assert_eq!(app.thumbnail_resume_meter(idx), None);
        app.settings.thumb_show_resume_meter = true;
        assert_eq!(app.thumbnail_resume_meter(idx), Some(1.0));

        app.settings.video_watched_to_end.remove(&key);
        assert_eq!(app.thumbnail_resume_meter(idx), None);
        app.image_metas[idx] = Some((100, 2048));
        app.settings
            .video_resume_positions
            .insert(key.clone(), 25.0);
        assert_eq!(app.thumbnail_resume_meter(idx), Some(0.25));
        app.settings.video_watched_to_end.insert(key.clone());
        assert_eq!(app.thumbnail_resume_meter(idx), Some(1.0));
        app.settings.video_watched_to_end.remove(&key);
        assert_eq!(app.thumbnail_resume_meter(idx), Some(0.25));
    }
    assert!(
        app.details_meta_pending.is_none(),
        "display never starts a probe"
    );
}

#[test]
fn media_resume_meter_watched_does_not_override_book_ordinal() {
    let mut app = media_app();
    app.book_resume_writer
        .as_ref()
        .unwrap()
        .read_all()
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap()
        .unwrap();
    app.poll_book_resume_meters(&egui::Context::default());
    let book = app.tmp.path().join("ordinal-book");
    app.install_new_items(vec![GridItem::Folder(book.clone())], vec![None]);
    app.settings
        .video_watched_to_end
        .insert(crate::adjustment_db::normalize_path(&book));
    app.book_resume_meters
        .record(&book, crate::book_resume_db::ReadingMeterValue::new(2, 4));
    assert_eq!(app.thumbnail_resume_meter(0), Some(0.5));
    app.book_resume_meters
        .record(&book, crate::book_resume_db::ReadingMeterValue::new(3, 4));
    assert_eq!(app.thumbnail_resume_meter(0), Some(0.75));
    app.settings.thumb_show_resume_meter = false;
    assert_eq!(app.thumbnail_resume_meter(0), None);
}

#[test]
fn media_resume_meter_watched_only_opens_video_and_audio_from_start() {
    use crate::settings::ResumeMode::{FromStart, Resume};

    let mut app = media_app();
    for idx in [0, 1] {
        let key = media_key(&app, idx);
        app.settings.video_watched_to_end.insert(key.clone());
        assert_eq!(app.thumbnail_resume_meter(idx), Some(1.0));
        let saved = app.settings.video_resume_positions.get(&key).copied();
        assert_eq!(saved, None);
        for from_grid in [false, true] {
            for open_starts_from_beginning in [false, true] {
                for nav_resume in [FromStart, Resume] {
                    assert_eq!(
                        super::video_resume_for_open(
                            saved,
                            from_grid,
                            open_starts_from_beginning,
                            nav_resume,
                        ),
                        None,
                        "watched is display metadata, so no resume seek is requested"
                    );
                }
            }
        }
    }
}

#[test]
fn media_resume_meter_watched_rename_and_purge_follow_exact_and_folder_boundaries() {
    let mut app = media_app();
    let old = PathBuf::from(r"C:\clips\watched.mp4");
    let new = PathBuf::from(r"C:\clips\renamed.mp4");
    let old_key = crate::adjustment_db::normalize_path(&old);
    let new_key = crate::adjustment_db::normalize_path(&new);
    let neighbor = format!("{old_key}.backup");
    app.settings.video_watched_to_end = HashSet::from([old_key.clone(), neighbor.clone()]);
    app.migrate_video_resume_positions_for_renamed_path(&old, &new);
    assert_eq!(
        app.settings.video_watched_to_end,
        HashSet::from([new_key.clone(), neighbor.clone()])
    );
    app.purge_video_resume_positions_for_removed_paths(&[new]);
    assert_eq!(app.settings.video_watched_to_end, HashSet::from([neighbor]));

    let old = PathBuf::from(r"D:\clips\Trip");
    let new = PathBuf::from(r"D:\clips\Trip 2026");
    let old_key = crate::adjustment_db::normalize_path(&old);
    let new_key = crate::adjustment_db::normalize_path(&new);
    let old_neighbor = format!("{old_key}2/other.mp4");
    let new_neighbor = format!("{new_key}2/other.flac");
    app.settings.video_watched_to_end = HashSet::from([
        old_key.clone(),
        format!("{old_key}/child.flac"),
        format!("{old_key}::entry.mp4"),
        old_neighbor.clone(),
        new_neighbor.clone(),
    ]);
    app.migrate_video_resume_positions_for_renamed_path(&old, &new);
    assert_eq!(
        app.settings.video_watched_to_end,
        HashSet::from([
            new_key.clone(),
            format!("{new_key}/child.flac"),
            format!("{new_key}::entry.mp4"),
            old_neighbor.clone(),
            new_neighbor.clone(),
        ])
    );
    app.purge_video_resume_positions_for_removed_paths(&[new]);
    assert_eq!(
        app.settings.video_watched_to_end,
        HashSet::from([old_neighbor, new_neighbor])
    );
}

#[test]
fn media_resume_meter_watched_rename_preserves_destination_media_state() {
    let mut app = media_app();
    let old = PathBuf::from(r"C:\clips\old");
    let new = PathBuf::from(r"C:\clips\new");
    let old_key = crate::adjustment_db::normalize_path(&old);
    let new_key = crate::adjustment_db::normalize_path(&new);
    let watched_source = format!("{old_key}/watched.mp4");
    let resume_source = format!("{old_key}/resume.flac");
    let resumed_destination = format!("{new_key}/watched.mp4");
    let watched_destination = format!("{new_key}/resume.flac");
    app.settings.video_watched_to_end =
        HashSet::from([watched_source, watched_destination.clone()]);
    app.settings.video_resume_positions = std::collections::HashMap::from([
        (resume_source, 25.0),
        (resumed_destination.clone(), 50.0),
    ]);

    app.migrate_video_resume_positions_for_renamed_path(&old, &new);

    assert_eq!(
        app.settings.video_watched_to_end,
        HashSet::from([watched_destination])
    );
    assert_eq!(
        app.settings.video_resume_positions,
        std::collections::HashMap::from([(resumed_destination, 50.0)])
    );
}

#[test]
fn media_resume_meter_delete_retry_poll_removes_only_confirmed_watched_scopes() {
    let mut app = media_app();
    let root = PathBuf::from(r"D:\clips\Trip");
    let root_key = crate::adjustment_db::normalize_path(&root);
    let exact = PathBuf::from(r"C:\clips\removed.mp4");
    let exact_key = crate::adjustment_db::normalize_path(&exact);
    let exact_neighbor = format!("{exact_key}.backup");
    let folder_neighbor = format!("{root_key}2/other.mp4");
    app.settings.video_watched_to_end = HashSet::from([
        root_key.clone(),
        format!("{root_key}/child.flac"),
        format!("{root_key}::entry.mp4"),
        folder_neighbor.clone(),
        exact_key.clone(),
        exact_neighbor.clone(),
    ]);
    let ctx = egui::Context::default();
    let (tx, rx) = std::sync::mpsc::channel();
    app.delete_purge_retry_pending = Some(crate::metadata_cleanup::DeletePurgeRetryPending { rx });
    tx.send(crate::metadata_cleanup::DeletePurgeRetryReport {
        removed_paths: vec![exact],
        remaining: 1,
        ..Default::default()
    })
    .unwrap();

    app.poll_delete_purge_retry(&ctx);

    assert!(!app.settings.video_watched_to_end.contains(&exact_key));
    assert!(app.settings.video_watched_to_end.contains(&root_key));
    assert!(app.settings.video_watched_to_end.contains(&exact_neighbor));
    assert!(app.delete_purge_retry_pending.is_none());
    assert!(app.delete_purge_retry_needed);

    let (tx, rx) = std::sync::mpsc::channel();
    app.delete_purge_retry_pending = Some(crate::metadata_cleanup::DeletePurgeRetryPending { rx });
    tx.send(crate::metadata_cleanup::DeletePurgeRetryReport {
        removed_paths: vec![root],
        ..Default::default()
    })
    .unwrap();

    app.poll_delete_purge_retry(&ctx);

    assert_eq!(
        app.settings.video_watched_to_end,
        HashSet::from([folder_neighbor, exact_neighbor])
    );
    assert!(app.delete_purge_retry_pending.is_none());
    assert!(!app.delete_purge_retry_needed);
}

#[test]
#[cfg(windows)]
fn media_resume_meter_exit_harvest_marks_active_and_parked_eof_without_dropping_contexts() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let active_path = PathBuf::from(r"C:\clips\watched-active.flac");
    let parked_path = PathBuf::from(r"C:\clips\watched-parked.mp4");
    let active_key = crate::adjustment_db::normalize_path(&active_path);
    let parked_key = crate::adjustment_db::normalize_path(&parked_path);
    let active_player = crate::video::VideoPlayer::disconnected_for_test(active_path.clone(), 13.0);
    active_player.mark_eof_for_test(0.0);
    app.build_active_context_for_test(None, super::DetachedSource::Audio, move |active| {
        active.items.push(GridItem::Audio(active_path));
        active.fullscreen_idx = Some(0);
        active.fs_cache.insert(
            0,
            super::FsCacheEntry::Video {
                player: Box::new(active_player),
                load_seq: 0,
            },
        );
    });
    let parked_player = crate::video::VideoPlayer::disconnected_for_test(parked_path.clone(), 21.0);
    parked_player.mark_eof_for_test(0.0);
    app.push_window_context_for_test(&ctx, 107, move |parked| {
        parked.items.push(GridItem::Video(parked_path));
        parked.fullscreen_idx = Some(0);
        parked.fs_cache.insert(
            0,
            super::FsCacheEntry::Video {
                player: Box::new(parked_player),
                load_seq: 0,
            },
        );
    });
    app.settings
        .video_resume_positions
        .insert(active_key.clone(), 13.0);
    app.settings
        .video_resume_positions
        .insert(parked_key.clone(), 21.0);

    app.save_detached_video_resume_positions_for_exit();

    for key in [active_key, parked_key] {
        assert!(!app.settings.video_resume_positions.contains_key(&key));
        assert!(app.settings.video_watched_to_end.contains(&key));
    }
    assert!(app.active_viewer_context_id().is_some());
    assert!(app.locate_window_context(107).is_some());
}
