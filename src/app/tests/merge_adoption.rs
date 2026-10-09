use super::*;

fn reading_shell_with_audio() -> (phase_c_support::AppTestEnv, PathBuf, PathBuf) {
    let mut app = phase_c_support::setup_app();
    app.active_quick_folder_slot = None;
    let origin = app.tmp.path().join("origin");
    std::fs::create_dir(&origin).unwrap();
    app.load_folder(origin.clone());
    let audio = app.tmp.path().join("song.flac");
    let cover = app.tmp.path().join("song.jpg");
    std::fs::write(&audio, b"audio").unwrap();
    std::fs::write(&cover, b"sidecar").unwrap();
    app.settings.skip_image_if_video_exists = true;
    app.settings.video_thumb_use_sidecar_image = true;
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
    assert!(app.items_are_reading_history_view);
    assert!(app.items.is_empty());
    assert!(
        app.reading_history_sources_pending
            .contains_key(&app.projected_viewer_context_id())
    );
    (app, audio, cover)
}

fn finish_sources(app: &mut App) {
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
}

#[test]
fn reading_hydration_preserves_adopted_surface_facet_history_and_pending_back() {
    let (mut app, audio, cover) = reading_shell_with_audio();
    let ctx = egui::Context::default();
    let origin = app.tmp.path().join("origin");
    app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
    let navigation = match app
        .top_level_grid_view
        .history_navigation_transition()
        .unwrap()
    {
        HistoryNavigationTransition::Physical(request) => request.navigation.clone(),
        _ => panic!("expected physical Back"),
    };
    let lease = app.top_level_grid_view.generation();
    let back = app.folder_nav_back_stack.clone();
    let forward = app.folder_nav_forward_stack.clone();
    let facet = app.facet_navigation.route().clone();
    let old_generation = app.items_generation;

    finish_sources(&mut app);

    assert!(matches!(&app.items[..], [GridItem::Audio(path)] if path == &audio));
    assert_eq!(
        app.video_thumb_overrides
            .get(&crate::path_key::normalize_keep_drive(&audio)),
        Some(&cover)
    );
    assert_eq!(app.top_level_grid_view.generation(), lease);
    assert_eq!(app.folder_nav_back_stack, back);
    assert_eq!(app.folder_nav_forward_stack, forward);
    assert_eq!(app.facet_navigation.route(), &facet);
    assert!(app.items_generation > old_generation);
    assert!(app.main_list_navigation_is_current(&navigation));
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_some()
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app
        .top_level_grid_view
        .history_navigation_transition()
        .is_some()
    {
        app.poll_collection_history_transition(&ctx);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(app.current_folder.as_ref(), Some(&origin));
    assert!(!app.items_are_reading_history_view);
}

#[test]
fn reading_hydration_preserves_unsubmitted_query_and_facet() {
    let (mut app, audio, _) = reading_shell_with_audio();
    app.show_search_bar = true;
    app.search_query = "song".into();

    app.settings.facet_filter.exts.insert("flac".into());
    let filter = app.settings.facet_filter.clone();
    finish_sources(&mut app);
    assert!(matches!(&app.items[..], [GridItem::Audio(path)] if path == &audio));
    assert_eq!(app.search_query, "song");
    assert!(app.search_filter.is_none());
    assert!(app.search_pending.is_none());
    assert_eq!(app.settings.facet_filter, filter);
    assert_eq!(app.visible_indices, vec![0]);
}

#[test]
fn reading_source_reply_after_reselect_cannot_fill_new_folder() {
    let (mut app, _, _) = reading_shell_with_audio();
    let next = app.tmp.path().join("next");
    std::fs::create_dir(&next).unwrap();
    app.load_folder(next.clone());
    finish_sources(&mut app);
    assert_eq!(app.current_folder.as_ref(), Some(&next));
    assert!(!app.items_are_reading_history_view);
    assert!(app.items.is_empty());
}

#[test]
fn folder_adoption_prunes_audio_absence_on_its_new_worker_queue() {
    let mut app = phase_c_support::setup_app();
    app.active_quick_folder_slot = None;
    app.settings.sidecar_backup_enabled = true;
    let folder = app.tmp.path().join("audio-folder");
    let sibling = app.tmp.path().join("sibling");
    std::fs::create_dir(&folder).unwrap();
    std::fs::create_dir(&sibling).unwrap();
    let cache_dir = crate::catalog::default_cache_dir();
    let admission = crate::catalog::CatalogAccess::for_cache_dir(&cache_dir).admit();
    let scope = crate::catalog::AudioArtCatalogScope::new(&folder);
    let sibling_scope = crate::catalog::AudioArtCatalogScope::new(&sibling);
    let key = scope.key_for(&folder.join("removed.mp3"));
    let sibling_key = sibling_scope.key_for(&sibling.join("removed.mp3"));
    let stamp = crate::catalog::AudioArtSourceStamp {
        mtime_secs: 123,
        file_size: 456,
    };
    for (scope, key) in [(&scope, &key), (&sibling_scope, &sibling_key)] {
        assert!(
            crate::catalog::save_audio_art_absence_with_cancel_check(
                &cache_dir,
                scope,
                key,
                stamp,
                admission,
                &|| false,
            )
            .unwrap()
        );
    }
    app.load_folder(folder.clone());
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        app.poll_sidecar_restore(&ctx);
        if matches!(
            crate::catalog::lookup_audio_art(&cache_dir, &scope, &key, stamp, admission).unwrap(),
            crate::catalog::AudioArtCached::Miss
        ) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "adopted folder never pruned its stale audio marker"
        );
        std::thread::yield_now();
    }
    assert_eq!(app.current_folder.as_ref(), Some(&folder));
    assert!(matches!(
        crate::catalog::lookup_audio_art(
            &cache_dir,
            &sibling_scope,
            &sibling_key,
            stamp,
            admission
        )
        .unwrap(),
        crate::catalog::AudioArtCached::NoArt
    ));
}

fn submit_shell_search(app: &mut App) -> Arc<AtomicBool> {
    app.show_search_bar = true;
    app.search_query = "song".into();
    app.search_target =
        crate::fts_index::SearchTarget::Only(vec![crate::fts_index::SourceKind::Filename]);
    app.execute_search(&egui::Context::default());
    Arc::clone(&app.search_pending.as_ref().unwrap().cancel)
}

fn finish_search(app: &mut App) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.search_pending.is_some() {
        app.poll_search(&ctx);
        assert!(std::time::Instant::now() < deadline, "search timed out");
        std::thread::yield_now();
    }
}

#[test]
fn reading_hydration_reexecutes_completed_shell_search() {
    let (mut app, _, _) = reading_shell_with_audio();
    submit_shell_search(&mut app);
    finish_search(&mut app);
    assert_eq!(app.search_filter, Some(HashSet::new()));
    finish_sources(&mut app);
    finish_search(&mut app);
    assert_eq!(app.search_query, "song");
    assert_eq!(app.search_filter, Some(HashSet::from([0])));
    assert_eq!(app.visible_indices, vec![0]);
}

#[test]
fn reading_hydration_retires_pending_shell_search_before_new_rows_publish() {
    let (mut app, _, _) = reading_shell_with_audio();
    let old_cancel = submit_shell_search(&mut app);
    // The real search owns the empty snapshot; leave its completion unpolled until
    // the independent real source-discovery worker hydrates the destination rows.
    finish_sources(&mut app);
    finish_search(&mut app);
    assert_eq!(app.search_query, "song");
    assert_eq!(app.search_filter, Some(HashSet::from([0])));
    assert_eq!(app.visible_indices, vec![0]);
    assert!(old_cancel.load(Ordering::Relaxed));
}
