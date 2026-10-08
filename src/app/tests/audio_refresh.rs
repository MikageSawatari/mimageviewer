use super::*;

fn begin_audio_refresh(app: &mut App) {
    app.refresh_current_view_pin_thumbnails(CurrentViewRefresh::Full);
    let owner = app.projected_viewer_context_id();
    // Observe the real worker before UI adoption; a refresh cannot mutate the mounted map.
    let result = app.current_view_pin_refreshes[&owner]
        .rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    let (tx, rx) = mpsc::channel();
    tx.send(result).unwrap();
    app.current_view_pin_refreshes.get_mut(&owner).unwrap().rx = rx;
}

#[test]
fn audio_synthetic_refresh_adopts_sidecars_and_retires_only_its_thumbnail_owner() {
    let mut app = phase_c_support::setup_app();
    let parent = app.tmp.path().join("audio-refresh");
    std::fs::create_dir(&parent).unwrap();
    let audio = parent.join("song.flac");
    let cover = parent.join("song.png");
    std::fs::write(&audio, b"audio").unwrap();
    std::fs::write(&cover, b"sidecar").unwrap();
    app.settings.skip_image_if_video_exists = true;
    app.settings.video_thumb_use_sidecar_image = true;
    app.current_folder = Some(search_results_synthetic_path());
    app.items_are_tag_view = true;
    app.items = vec![GridItem::Audio(audio.clone())];
    // History and bookmark display stamps are not source stamps.
    app.image_metas = vec![Some((0, 999))];
    app.thumbnails = vec![ThumbnailState::NoArt];
    let original_cancel = app.cancel_token.clone();
    let sibling = app.build_window_context_for_test(91_347, |mounted| {
        mounted.items = vec![GridItem::Audio(audio.clone())];
        mounted.thumbnails = vec![ThumbnailState::NoArt];
    });
    let sibling_cancel = app
        .with_viewer_context(sibling, |mounted| mounted.cancel_token.clone())
        .unwrap();
    let key = crate::path_key::normalize_keep_drive(&audio);
    begin_audio_refresh(&mut app);
    assert!(app.video_thumb_overrides.is_empty());
    assert!(matches!(app.thumbnails[0], ThumbnailState::NoArt));
    app.poll_current_view_pin_refresh();
    assert_eq!(app.video_thumb_overrides.get(&key), Some(&cover));
    assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
    assert!(original_cancel.load(Ordering::Relaxed));
    assert!(!sibling_cancel.load(Ordering::Relaxed));
    app.with_viewer_context(sibling, |mounted| {
        assert!(matches!(mounted.thumbnails[0], ThumbnailState::NoArt));
        assert!(mounted.video_thumb_overrides.is_empty());
    })
    .unwrap();
    std::fs::remove_file(&cover).unwrap();
    app.thumbnails[0] = ThumbnailState::NoArt;
    begin_audio_refresh(&mut app);
    assert_eq!(app.video_thumb_overrides.get(&key), Some(&cover));
    app.poll_current_view_pin_refresh();
    assert!(app.video_thumb_overrides.is_empty());
    assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
}

#[test]
fn audio_synthetic_refresh_rejects_a_result_from_an_old_items_generation() {
    let mut app = phase_c_support::setup_app();
    app.items = vec![GridItem::Audio(app.tmp.path().join("new.mp3"))];
    app.thumbnails = vec![ThumbnailState::NoArt];
    let owner = app.projected_viewer_context_id();
    let old_cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    tx.send((
        None,
        Some(std::collections::HashMap::from([(
            "old.mp3".into(),
            PathBuf::from("old.png"),
        )])),
    ))
    .unwrap();
    let old_generation = app.items_generation.wrapping_add(1);
    app.current_view_pin_refreshes.insert(
        owner,
        CurrentViewPinRefresh {
            rx,
            cancel: old_cancel.clone(),
            items_generation: old_generation,
            refresh: CurrentViewRefresh::Full,
        },
    );
    let active_cancel = app.cancel_token.clone();
    app.poll_current_view_pin_refresh();
    assert!(app.video_thumb_overrides.is_empty());
    assert!(matches!(app.thumbnails[0], ThumbnailState::NoArt));
    assert!(old_cancel.load(Ordering::Relaxed));
    assert!(!active_cancel.load(Ordering::Relaxed));
    assert!(app.current_view_pin_refreshes.is_empty());
}

#[test]
fn audio_catalog_prune_is_not_a_cell_during_ranking_eviction_or_prefetch_suppression() {
    let mut app = phase_c_support::setup_app();
    let parent = app.tmp.path().join("prune-owner");
    app.items = vec![GridItem::Audio(parent.join("song.mp3"))];
    app.image_metas = vec![None];
    app.thumbnails = vec![ThumbnailState::NoArt];
    app.visible_indices = vec![0];
    let cache_dir = app.tmp.path().join("cache");
    let admission = crate::catalog::CatalogAccess::for_cache_dir(&cache_dir).admit();
    let mut prune = crate::thumb_loader::LoadRequest {
        // A catalog job has no grid index, even if the unused field is an extreme value.
        idx: usize::MAX,
        priority: true,
        raw_source: crate::thumb_loader::LoadRequestSource::AudioArtPrune {
            parent,
            inventory: Arc::new(Default::default()),
            cache_dir,
            admission,
        },
        ..Default::default()
    };
    assert_eq!(prune.grid_index(), None);
    assert_eq!(
        crate::thumb_loader::request_worker_priority_key(&prune, 0, 0),
        (2, 0, 0)
    );
    let visible = crate::thumb_loader::LoadRequest {
        priority: true,
        ..Default::default()
    };
    let prefetch = crate::thumb_loader::LoadRequest::default();
    assert!(
        crate::thumb_loader::request_worker_priority_key(&visible, 0, 0)
            < crate::thumb_loader::request_worker_priority_key(&prune, 0, 0)
    );
    assert!(
        crate::thumb_loader::request_worker_priority_key(&prefetch, 0, 0)
            < crate::thumb_loader::request_worker_priority_key(&prune, 0, 0)
    );
    prune.idx = 0;
    let queue = Arc::new((Mutex::new(vec![prune]), Condvar::new()));
    app.heavy_io_queue = Some(queue.clone());
    app.evict_thumbnail_for_reload(0);
    assert_eq!(queue.0.lock().unwrap().len(), 1);
    app.thumbnails[0] = ThumbnailState::NoArt;
    app.install_thumbnail_keep_projection(thumbnail_keep_projection(1, [], [], None, [], []), true);
    assert_eq!(queue.0.lock().unwrap().len(), 1);
    app.update_keep_range_and_requests(&egui::Context::default(), std::time::Instant::now());
    assert_eq!(queue.0.lock().unwrap().len(), 1);
    assert!(queue.0.lock().unwrap()[0].grid_index().is_none());
    assert_eq!(app.cache_gen_done.load(Ordering::Relaxed), 0);
    assert!(app.requested.is_empty());
    assert!(app.rx.try_recv().is_err());
}
