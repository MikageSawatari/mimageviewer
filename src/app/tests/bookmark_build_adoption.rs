use super::*;

fn audio_sidecar(parent: &std::path::Path, name: &str) -> (PathBuf, PathBuf) {
    std::fs::create_dir_all(parent).unwrap();
    let audio = parent.join(format!("{name}.flac"));
    let cover = parent.join(format!("{name}.png"));
    std::fs::write(&audio, b"audio").unwrap();
    std::fs::write(&cover, b"sidecar").unwrap();
    (audio, cover)
}

/// Hold a result produced by the real DB/sidecar worker until the destination is installed.
/// Replacing only its receiver retains the pending request's production ownership evidence.
fn queue_finished_bookmark_build(app: &mut App) -> Arc<AtomicBool> {
    let pending = app
        .bookmark_browser_pending
        .as_mut()
        .expect("bookmark build");
    let result = pending
        .rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("bookmark worker finished")
        .expect("bookmark worker succeeded");
    assert!(!result.video_thumb_overrides.is_empty());
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(result)).unwrap();
    pending.rx = rx;
    Arc::clone(&pending.cancel)
}

fn bookmark_build_fixture() -> (
    phase_c_support::AppTestEnv,
    PathBuf,
    PathBuf,
    Arc<AtomicBool>,
) {
    let mut app = phase_c_support::setup_app();
    app.active_quick_folder_slot = None;
    app.settings.skip_image_if_video_exists = true;
    app.settings.video_thumb_use_sidecar_image = true;
    let (old_audio, _) = audio_sidecar(&app.tmp.path().join("old-bookmark"), "old");
    let (audio, cover) = audio_sidecar(&app.tmp.path().join("destination"), "song");
    app.video_bookmark_db
        .as_ref()
        .unwrap()
        .add(&old_audio, 12.0, Some("old bookmark"), &[])
        .unwrap();
    app.open_bookmark_browser();
    assert!(app.items_are_bookmark_view);
    let cancel = queue_finished_bookmark_build(&mut app);
    (app, audio, cover, cancel)
}

fn assert_bookmark_build_retired(
    app: &mut App,
    audio: &std::path::Path,
    cover: &std::path::Path,
    cancel: &AtomicBool,
) {
    assert!(!app.items_are_bookmark_view);
    let key = crate::path_key::normalize_keep_drive(audio);
    assert_eq!(
        app.video_thumb_overrides.get(&key).map(PathBuf::as_path),
        Some(cover)
    );
    let before = app.video_thumb_overrides.clone();
    let retired = app.bookmark_browser_pending.is_none();
    let generation = app.items_generation;
    let items = app.items.clone();
    app.poll_bookmark_browser(&egui::Context::default());
    assert_eq!(
        app.video_thumb_overrides, before,
        "a finished bookmark build must not replace the adopted destination's audio sidecars"
    );
    assert!(
        retired,
        "the visible adoption must retire the old bookmark worker"
    );
    assert!(cancel.load(Ordering::Relaxed));
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.items, items);
}

#[test]
fn bookmark_build_folder_adoption_retires_worker_and_preserves_audio_sidecar() {
    let (mut app, audio, cover, cancel) = bookmark_build_fixture();
    app.load_folder(audio.parent().unwrap().to_path_buf());
    assert!(matches!(&app.items[..], [GridItem::Audio(path)] if path == &audio));
    assert_bookmark_build_retired(&mut app, &audio, &cover, &cancel);
}

#[test]
fn bookmark_build_reading_adoption_retires_worker_and_preserves_audio_sidecar() {
    let (mut app, audio, cover, cancel) = bookmark_build_fixture();
    app.reading_history_db
        .as_ref()
        .unwrap()
        .upsert(
            crate::reading_history_db::ReadingHistoryEntry::new(
                audio.clone(),
                crate::reading_history_db::ReadingHistoryKind::Audio,
                None,
                "song.flac".into(),
                None,
                None,
            ),
            100,
        )
        .unwrap();
    app.enter_reading_history_from_menu();
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app
        .reading_history_sources_pending
        .contains_key(&app.projected_viewer_context_id())
    {
        app.poll_reading_history_thumbnail_sources(&ctx);
        assert!(
            std::time::Instant::now() < deadline,
            "reading sources timed out"
        );
        std::thread::yield_now();
    }
    assert!(app.items_are_reading_history_view);
    assert_bookmark_build_retired(&mut app, &audio, &cover, &cancel);
}

#[test]
fn bookmark_build_rating_adoption_retires_worker_and_preserves_audio_sidecar() {
    let (mut app, audio, cover, cancel) = bookmark_build_fixture();
    let key = crate::adjustment_db::normalize_path(&audio);
    let metadata = crate::rating_db::RatingMeta::new(crate::rating_db::RatingItemKind::Audio)
        .with_source_path(&audio);
    app.rating_db
        .as_ref()
        .unwrap()
        .set_user_rating(&key, 3, Some(&metadata))
        .unwrap();
    app.enter_rating_view_from_menu(3);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.rating_view_pending.is_some() {
        app.poll_rating_view();
        assert!(
            std::time::Instant::now() < deadline,
            "rating sources timed out"
        );
        std::thread::yield_now();
    }
    assert!(app.items_are_rating_view);
    assert_bookmark_build_retired(&mut app, &audio, &cover, &cancel);
}

#[test]
fn bookmark_build_same_rows_refresh_keeps_mounted_grid_and_sources() {
    let (mut app, _, _, _) = bookmark_build_fixture();
    let ctx = egui::Context::default();
    app.poll_bookmark_browser(&ctx);
    assert_eq!(app.bookmark_browser_rows.len(), 1);
    assert!(matches!(&app.items[..], [GridItem::Audio(_)]));
    let items = app.items.clone();
    let sources = app.video_thumb_overrides.clone();
    let generation = app.items_generation;
    app.refresh_bookmark_browser();
    queue_finished_bookmark_build(&mut app);
    app.poll_bookmark_browser(&ctx);
    assert!(app.bookmark_browser_pending.is_none());
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.items, items);
    assert_eq!(app.video_thumb_overrides, sources);
}

#[test]
fn bookmark_build_completed_refresh_survives_same_bookmark_sort_and_hydrates_sources() {
    let (mut app, _, _, _) = bookmark_build_fixture();
    let ctx = egui::Context::default();
    app.poll_bookmark_browser(&ctx);
    let old_audio = app.bookmark_browser_rows[0].source_path().to_path_buf();
    let old_cover = old_audio.with_extension("png");
    let new_cover = old_audio.with_extension("jpg");
    std::fs::remove_file(&old_cover).unwrap();
    std::fs::write(&new_cover, b"replacement sidecar").unwrap();
    app.refresh_bookmark_browser();
    let cancel = queue_finished_bookmark_build(&mut app);
    let old_surface_generation = app.top_level_grid_view.generation();
    app.set_bookmark_view_sort(crate::bookmark_browser::BookmarkViewSort::CreatedAtAsc);
    assert_ne!(app.top_level_grid_view.generation(), old_surface_generation);
    assert!(
        app.bookmark_browser_pending.is_some(),
        "sort must keep the same owner's build"
    );
    assert!(!cancel.load(Ordering::Relaxed));
    let key = crate::path_key::normalize_keep_drive(&old_audio);
    assert_eq!(app.video_thumb_overrides.get(&key), Some(&old_cover));
    app.poll_bookmark_browser(&ctx);
    assert!(app.bookmark_browser_pending.is_none());
    assert!(app.items_are_bookmark_view);
    assert_eq!(app.bookmark_browser_rows.len(), 1);
    assert_eq!(app.video_thumb_overrides.get(&key), Some(&new_cover));
    assert_eq!(
        app.bookmark_view_sort,
        crate::bookmark_browser::BookmarkViewSort::CreatedAtAsc
    );
}

#[test]
fn bookmark_build_late_owner_proof_rejects_held_reply_after_folder_adoption() {
    let (mut app, audio, cover, cancel) = bookmark_build_fixture();
    // Hold the request outside App to bypass eager retirement and exercise poll's proof check.
    let held = app.bookmark_browser_pending.take().unwrap();
    app.load_folder(audio.parent().unwrap().to_path_buf());
    assert!(!cancel.load(Ordering::Relaxed));
    app.bookmark_browser_pending = Some(held);
    let key = crate::path_key::normalize_keep_drive(&audio);
    assert_eq!(app.video_thumb_overrides.get(&key), Some(&cover));
    let sources = app.video_thumb_overrides.clone();
    let items = app.items.clone();
    let generation = app.items_generation;
    app.poll_bookmark_browser(&egui::Context::default());
    assert!(app.bookmark_browser_pending.is_none());
    assert!(cancel.load(Ordering::Relaxed));
    assert_eq!(app.video_thumb_overrides, sources);
    assert_eq!(app.items, items);
    assert_eq!(app.items_generation, generation);
}

#[test]
fn bookmark_build_clear_quick_slots_replaces_old_pending_and_finishes_loading() {
    let (mut app, _, _, cancel) = bookmark_build_fixture();
    let items = app.items.clone();
    let sources = app.video_thumb_overrides.clone();
    let generation = app.items_generation;
    app.execute_clear_quick_folder_slots();
    assert!(
        app.bookmark_browser_pending.is_some(),
        "clearing slot ownership must continue loading the still-mounted Bookmark surface"
    );
    assert!(cancel.load(Ordering::Relaxed));
    assert_eq!(app.items, items);
    assert_eq!(app.video_thumb_overrides, sources);
    assert_eq!(app.items_generation, generation);
    assert!(app.items_are_bookmark_view);
    let new_cancel = queue_finished_bookmark_build(&mut app);
    assert!(!Arc::ptr_eq(&new_cancel, &cancel));
    assert!(!new_cancel.load(Ordering::Relaxed));
    app.poll_bookmark_browser(&egui::Context::default());
    assert!(app.bookmark_browser_pending.is_none());
    assert_eq!(app.bookmark_browser_rows.len(), 1);
    assert!(matches!(&app.items[..], [GridItem::Audio(_)]));
    assert!(!app.video_thumb_overrides.is_empty());
    assert_eq!(app.address, "ブックマーク");
}

#[test]
fn bookmark_build_detached_item_install_and_poll_preserve_main_pending() {
    let (mut app, audio, _, cancel) = bookmark_build_fixture();
    let main_items = app.items.clone();
    let main_sources = app.video_thumb_overrides.clone();
    let main_generation = app.items_generation;
    let sibling = app.build_window_context_for_test(91_348, |_| {});
    app.with_viewer_context(sibling, |mounted| {
        mounted.install_new_items(vec![GridItem::Audio(audio.clone())], vec![None]);
        mounted.poll_bookmark_browser(&egui::Context::default());
        assert!(
            mounted.bookmark_browser_pending.is_some(),
            "a mounted sibling must not retire main's request"
        );
        assert!(
            mounted.video_thumb_overrides.is_empty(),
            "a mounted sibling must not adopt main's reply"
        );
        assert!(!cancel.load(Ordering::Relaxed));
    })
    .unwrap();
    assert!(app.bookmark_browser_pending.is_some());
    assert_eq!(app.items, main_items);
    assert_eq!(app.video_thumb_overrides, main_sources);
    assert_eq!(app.items_generation, main_generation);
    app.poll_bookmark_browser(&egui::Context::default());
    assert!(app.bookmark_browser_pending.is_none());
    assert!(app.items_are_bookmark_view);
    assert_eq!(app.bookmark_browser_rows.len(), 1);
    assert!(!app.video_thumb_overrides.is_empty());
}
