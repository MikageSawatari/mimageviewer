use super::*;

fn indexed_video(path: PathBuf) -> crate::search_index_db::IndexEntry {
    crate::search_index_db::IndexEntry {
        display_name: path.file_name().unwrap().to_string_lossy().into_owned(),
        path,
        kind: crate::search_index_db::IndexKind::VideoFile,
        mtime: 7,
    }
}

fn prepare_settings() -> crate::settings::Settings {
    crate::settings::Settings {
        skip_image_if_video_exists: true,
        video_thumb_use_sidecar_image: true,
        ..Default::default()
    }
}

#[test]
fn favorite_search_sidecars_use_full_source_paths_and_the_released_off_setting() {
    let tmp = tempfile::tempdir().unwrap();
    let mut entries = Vec::new();
    let mut covers = Vec::new();
    for folder in ["one", "two"] {
        let parent = tmp.path().join(folder);
        std::fs::create_dir(&parent).unwrap();
        let video = parent.join("same.mp4");
        let cover = parent.join("same.jpg");
        std::fs::write(&video, b"video").unwrap();
        std::fs::write(&cover, b"sidecar").unwrap();
        entries.push(indexed_video(video));
        covers.push(cover);
    }
    let cancel = AtomicBool::new(false);
    let mut settings = prepare_settings();
    let prepared = prepare_favsearch_results(entries.clone(), &settings, &cancel).unwrap();
    assert_eq!(prepared.entries.len(), 2);
    assert_eq!(prepared.video_thumb_overrides.len(), 2);
    for (entry, cover) in entries.iter().zip(&covers) {
        let key = crate::path_key::normalize_keep_drive(&entry.path);
        assert_eq!(prepared.video_thumb_overrides.get(&key), Some(cover));
    }
    assert!(!prepared.video_thumb_overrides.contains_key("same"));
    settings.video_thumb_use_sidecar_image = false;
    let prepared = prepare_favsearch_results(entries, &settings, &cancel).unwrap();
    assert_eq!(prepared.entries.len(), 2);
    assert!(prepared.video_thumb_overrides.is_empty());
}

#[test]
fn canceled_favorite_search_does_not_publish_prepared_sources() {
    let canceled = AtomicBool::new(true);
    assert!(prepare_favsearch_results(Vec::new(), &prepare_settings(), &canceled).is_none());
}

fn finish_favorite_search(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.favsearch_pending.is_some() {
        app.poll_favsearch();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn favorite_search_worker_installs_sources_before_loading_and_requeries_on_refresh() {
    let mut app = phase_c_support::setup_app();
    let root = app.tmp.path().join("favorite-sidecars");
    std::fs::create_dir(&root).unwrap();
    let video = root.join("track.mp4");
    let cover = root.join("track.jpg");
    std::fs::write(&video, b"video").unwrap();
    std::fs::write(&cover, b"sidecar").unwrap();
    let db = crate::search_index_db::SearchIndexDb::open_at(&root.join("index.db")).unwrap();
    db.upsert_children(&root, &root, &[indexed_video(video.clone())])
        .unwrap();
    app.search_index_db = Some(Arc::new(db));
    let mut favorite = crate::settings::FavoriteEntry::new("sidecars".into(), root);
    favorite.auto_index_structure = true;
    app.settings.favorites.push(favorite);
    app.settings.skip_image_if_video_exists = true;
    app.settings.video_thumb_use_sidecar_image = true;
    app.favsearch.active = true;
    app.favsearch.query = "track".into();
    // Released Ctrl+S is container-only; legacy video rows are queried only explicitly.
    assert!(
        app.search_index_db
            .as_ref()
            .unwrap()
            .search_with_epub(
                "track",
                &[app.settings.favorites.last().unwrap().path.clone()],
                None,
                crate::search_query::MatchMode::And,
                true,
            )
            .unwrap()
            .is_empty()
    );
    app.favsearch.kind_filter = Some(crate::search_index_db::IndexKind::VideoFile);
    app.execute_favsearch();
    finish_favorite_search(&mut app);
    let key = crate::path_key::normalize_keep_drive(&video);
    assert_eq!(app.video_thumb_overrides.get(&key), Some(&cover));
    assert!(matches!(&app.items[..], [GridItem::Video(path)] if path == &video));

    std::fs::remove_file(&cover).unwrap();
    app.execute_favsearch();
    finish_favorite_search(&mut app);
    assert!(!app.video_thumb_overrides.contains_key(&key));
    assert!(matches!(&app.items[..], [GridItem::Video(path)] if path == &video));

    app.video_thumb_overrides.insert(key, cover);
    app.favsearch.query.clear();
    app.execute_favsearch();
    assert!(app.items.is_empty());
    assert!(app.video_thumb_overrides.is_empty());
}
