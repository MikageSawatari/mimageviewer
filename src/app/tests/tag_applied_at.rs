use super::*;
use crate::settings::{DetailsSortKey, GridViewMode, SortOrder};
use crate::tag_view::{TagViewKindFilter, TagViewSort};

fn wait_for_tag_result(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.tag_view_pending.is_some() {
        app.poll_tag_view();
        assert!(
            std::time::Instant::now() < deadline,
            "tag worker did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        app.tag_view.reject_message.is_none(),
        "{:?}",
        app.tag_view.reject_message
    );
    // 実frameでは sidecar/page identity の準備も進む。次の操作を同じ条件で行う。
    app.finish_page_identity_preparation_for_test();
}

fn seed(app: &mut App) -> (PathBuf, PathBuf) {
    let source = crate::data_dir::get().join("source");
    std::fs::create_dir_all(&source).unwrap();
    app.current_folder = Some(source);
    let image = app.current_folder.as_ref().unwrap().join("a.jpg");
    let folder = app.current_folder.as_ref().unwrap().join("z-folder");
    std::fs::write(&image, b"image").unwrap();
    std::fs::create_dir(&folder).unwrap();
    let path = crate::data_dir::get().join("tags.db");
    let mut db = crate::tags_db::TagsDb::open_at(&path).unwrap();
    for item in [&image, &folder] {
        db.set_item_tags(&crate::tags_db::item_key_for_path(item), ["cat"], "test")
            .unwrap();
    }
    drop(db);
    let conn = rusqlite::Connection::open(path).unwrap();
    for (item, time) in [(&image, 10), (&folder, 20)] {
        conn.execute(
            "UPDATE item_tags SET applied_at=?1 WHERE item_key=?2",
            rusqlite::params![time, crate::tags_db::item_key_for_path(item)],
        )
        .unwrap();
    }
    (image, folder)
}

#[test]
fn tag_applied_at_menu_handler_requeries_and_preserves_path_default_and_shared_sort() {
    let mut app = phase_c_support::setup_app();
    let (image, folder) = seed(&mut app);
    app.settings.sort_order = SortOrder::FileNameDesc;
    app.open_tag_view_for_tag("cat");
    wait_for_tag_result(&mut app);
    assert_eq!(app.settings.tag_view_sort, TagViewSort::PathOrder);
    assert_eq!(
        app.items
            .iter()
            .map(GridItem::drag_source_path)
            .collect::<Vec<_>>(),
        [Some(image.as_path()), Some(folder.as_path())]
    );
    assert_eq!(app.tag_view_applied_at_ms(0), Some(10_000));
    assert_eq!(app.tag_view_applied_at_ms(1), Some(20_000));
    for (sort, first) in [
        (TagViewSort::AppliedAtDesc, &folder),
        (TagViewSort::AppliedAtAsc, &image),
        (TagViewSort::PathOrder, &image),
    ] {
        app.set_tag_view_sort(sort);
        assert!(app.tag_view_pending.is_some());
        assert_eq!(app.tag_view.last_executed, "#cat");
        assert_eq!(
            app.tag_view.last_executed_kind_filter,
            TagViewKindFilter::All
        );
        wait_for_tag_result(&mut app);
        assert_eq!(app.items[0].drag_source_path(), Some(first.as_path()));
        assert_eq!(app.settings.sort_order, SortOrder::FileNameDesc);
    }
    app.tag_view.kind_filter = TagViewKindFilter::Image;
    app.set_tag_view_sort(TagViewSort::AppliedAtDesc);
    wait_for_tag_result(&mut app);
    assert_eq!(app.items.len(), 1);
    assert_eq!(app.items[0].drag_source_path(), Some(image.as_path()));
    assert_eq!(
        app.tag_view.last_executed_kind_filter,
        TagViewKindFilter::Image
    );
}

#[test]
fn tag_applied_at_header_sort_is_scoped_and_toolbar_and_ring_agree() {
    let mut app = phase_c_support::setup_app();
    seed(&mut app);
    app.settings.grid_view_mode = GridViewMode::Details;
    app.settings.details_sort_key = DetailsSortKey::Name;
    app.open_tag_view_for_tag("cat");
    wait_for_tag_result(&mut app);
    assert_eq!(app.settings.details_sort_key, DetailsSortKey::Toolbar);
    assert_eq!(app.grid_sort_lock_reason(), None);
    assert!(app.grid_sort_refresh_available());
    app.set_details_sort_key(DetailsSortKey::AppliedAt);
    assert_eq!(app.details_order, [0, 1]);
    assert_eq!(
        app.grid_sort_lock_reason(),
        Some(GridSortLockReason::DetailsHeaderSort)
    );
    let picker = app.build_ring_picker_state(crate::ring_shortcut::RingShortcutContext::Grid);
    assert_eq!(
        picker.grid_sort_target,
        crate::ring_shortcut::RingPickerGridSortTarget::Locked(
            GridSortLockReason::DetailsHeaderSort
        )
    );
    app.set_details_sort_key(DetailsSortKey::AppliedAt);
    assert_eq!(app.details_order, [1, 0]);
    app.execute_tag_view();
    wait_for_tag_result(&mut app);
    assert_eq!(app.settings.details_sort_key, DetailsSortKey::AppliedAt);
    assert!(!app.settings.details_sort_ascending);
    assert_eq!(app.details_order, [1, 0]);
    app.set_details_sort_key(DetailsSortKey::AppliedAt);
    assert_eq!(app.grid_sort_lock_reason(), None);
    let picker = app.build_ring_picker_state(crate::ring_shortcut::RingShortcutContext::Grid);
    assert_eq!(
        picker.grid_sort_target,
        crate::ring_shortcut::RingPickerGridSortTarget::Locked(GridSortLockReason::TagViewOrder)
    );
    app.items_are_tag_view = false;
    assert_eq!(app.tag_view_applied_at_ms(0), None);
    app.set_details_sort_key(DetailsSortKey::AppliedAt);
    assert_eq!(app.settings.details_sort_key, DetailsSortKey::Toolbar);
}

#[test]
fn tag_applied_at_return_from_child_resets_header_owner() {
    let mut app = phase_c_support::setup_app();
    let (_, folder) = seed(&mut app);
    app.open_tag_view_for_tag("cat");
    wait_for_tag_result(&mut app);
    app.settings.tag_view_sort = TagViewSort::AppliedAtDesc;
    app.tag_view.nav_stack.push(folder);
    app.items_are_tag_view = false;
    assert_eq!(
        app.grid_sort_lock_reason(),
        Some(GridSortLockReason::FixedOrder)
    );
    app.settings.grid_view_mode = GridViewMode::Details;
    app.settings.details_sort_key = DetailsSortKey::Name;
    app.tag_view_back();
    assert_eq!(app.settings.details_sort_key, DetailsSortKey::Toolbar);
    wait_for_tag_result(&mut app);
    assert_eq!(app.settings.details_sort_key, DetailsSortKey::Toolbar);
    assert!(matches!(app.items[0], GridItem::Folder(_)));
    assert_eq!(app.grid_sort_lock_reason(), None);
}

#[test]
fn tag_applied_at_tag_edits_do_not_research_or_change_snapshot_times() {
    let mut app = phase_c_support::setup_app();
    let (image, _) = seed(&mut app);
    app.settings.tag_view_sort = TagViewSort::AppliedAtDesc;
    app.open_tag_view_for_tag("cat");
    wait_for_tag_result(&mut app);
    let times = app.tag_view_applied_times.clone();
    let keys = app.items.iter().map(GridItem::perf_key).collect::<Vec<_>>();
    let key = crate::tags_db::item_key_for_path(&image);
    app.set_tags_cache_entry(key, Vec::new());
    app.poll_tag_write_results();
    assert!(app.tag_view_pending.is_none());
    assert_eq!(app.tag_view_applied_times, times);
    assert_eq!(
        app.items.iter().map(GridItem::perf_key).collect::<Vec<_>>(),
        keys
    );
}

#[test]
fn tag_applied_at_header_ties_keep_full_path_order_in_both_directions() {
    let mut app = phase_c_support::setup_app();
    let first = app.tmp.path().join("a-parent/z.jpg");
    let second = app.tmp.path().join("z-parent/a.jpg");
    app.items = vec![
        GridItem::Image(first.clone()),
        GridItem::Image(second.clone()),
    ];
    app.image_metas = vec![Some((0, 0)); 2];
    app.thumbnails = vec![ThumbnailState::Pending; 2];
    app.items_are_tag_view = true;
    app.settings.grid_view_mode = GridViewMode::Details;
    for path in [&first, &second] {
        app.tag_view_applied_times
            .insert(crate::tags_db::item_key_for_path(path), 10_000);
    }
    app.rebuild_visible_indices();
    app.set_details_sort_key(DetailsSortKey::AppliedAt);
    assert_eq!(app.details_order, [0, 1]);
    app.set_details_sort_key(DetailsSortKey::AppliedAt);
    assert_eq!(app.details_order, [0, 1]);
}

#[test]
fn tag_applied_at_research_and_child_open_replace_the_previous_request_in_both_orders() {
    let mut app = phase_c_support::setup_app();
    let (_, folder) = seed(&mut app);
    app.open_tag_view_for_tag("cat");
    wait_for_tag_result(&mut app);

    app.set_tag_view_sort(TagViewSort::AppliedAtDesc);
    assert!(app.tag_view_pending.is_some());
    assert!(app.start_physical_history_transition(
        PhysicalHistoryIntent::Navigation {
            replay: None,
            auto_fullscreen: false
        },
        folder,
        StartupListIntent::ExplicitList,
    ));
    assert!(
        app.tag_view_pending.is_none(),
        "new child admission must retire the root search"
    );
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_some()
            || app.top_level_grid_view.open_path_classification().is_some()
    );

    // 同じ検索語のまま、後から受理した並べ替えが子の準備を置き換える。
    app.set_tag_view_sort(TagViewSort::AppliedAtAsc);
    assert!(app.tag_view_pending.is_some());
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_none()
    );
    assert!(app.top_level_grid_view.open_path_classification().is_none());
    wait_for_tag_result(&mut app);
    assert!(app.items_are_tag_view);
    assert_eq!(app.items.len(), 2);
}

#[test]
fn tag_applied_at_research_keeps_directly_opened_image_and_its_identity() {
    let mut app = phase_c_support::setup_app();
    let (image, _) = seed(&mut app);
    // Valid pixels let the real direct-open handler admit the image headlessly.
    image::RgbImage::new(2, 3).save(&image).unwrap();
    app.open_tag_view_for_tag("cat");
    wait_for_tag_result(&mut app);
    app.set_tag_view_sort(TagViewSort::AppliedAtDesc);
    app.open_fullscreen(0, HistoryTrigger::UserChosen);
    assert_eq!(app.fullscreen_idx, Some(0));
    app.move_grid_cursor(Some(0), GridCursorIntent::Automatic);
    app.text_mode = true;
    let old_workers = Arc::clone(&app.cancel_token);
    wait_for_tag_result(&mut app);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.selected, Some(1));
    assert!(!app.text_mode);
    assert!(old_workers.load(Ordering::Relaxed));
    assert!(!Arc::ptr_eq(&old_workers, &app.cancel_token));
    assert_eq!(app.items[1].drag_source_path(), Some(image.as_path()));

    // Query changes and sort-only searches share the same membership adoption rule.
    app.tag_view.query = "#ca".into();
    app.execute_tag_view();
    wait_for_tag_result(&mut app);
    assert_eq!(app.fullscreen_idx, Some(1));
    app.tag_view.query = "#absent".into();
    app.execute_tag_view();
    wait_for_tag_result(&mut app);
    assert_eq!(app.fullscreen_idx, None);
}

#[test]
fn tag_applied_at_research_keeps_media_player_allocation_and_remaps_its_slot() {
    for audio in [false, true] {
        let mut app = phase_c_support::setup_app();
        let (image, _) = seed(&mut app);
        let path = image.with_extension(if audio { "mp3" } else { "mp4" });
        std::fs::write(&path, b"media").unwrap();
        let mut db =
            crate::tags_db::TagsDb::open_at(&crate::data_dir::get().join("tags.db")).unwrap();
        db.set_item_tags(&crate::tags_db::item_key_for_path(&path), ["cat"], "test")
            .unwrap();
        drop(db);
        app.open_tag_view_for_tag("cat");
        wait_for_tag_result(&mut app);
        let old = app
            .items
            .iter()
            .position(|item| item.drag_source_path() == Some(path.as_path()))
            .unwrap();
        app.set_tag_view_sort(TagViewSort::AppliedAtDesc);
        // Use the existing disconnected player fixture: no decoder/window is launched.
        app.fullscreen_idx = Some(old);
        let player =
            Box::new(crate::video::VideoPlayer::stream_ready_disconnected_for_test(path.clone()));
        let original = std::ptr::from_ref(&*player);
        app.fs_cache.insert(
            old,
            FsCacheEntry::Video {
                player,
                load_seq: 19,
            },
        );
        if audio {
            app.video_audio_mode = Some(old);
        }
        wait_for_tag_result(&mut app);
        let new = app.fullscreen_idx.expect("media must stay open");
        assert_eq!(app.items[new].drag_source_path(), Some(path.as_path()));
        assert_ne!(old, new);
        let Some(FsCacheEntry::Video { player, load_seq }) = app.fs_cache.get(&new) else {
            panic!("player lost")
        };
        assert_eq!(std::ptr::from_ref(&**player), original);
        assert_eq!(*load_seq, 19);
        if audio {
            assert_eq!(app.video_audio_mode, Some(new));
        }
    }
}

#[test]
fn tag_applied_at_snapshot_retires_search_request_columns_and_return_origin() {
    let mut app = phase_c_support::setup_app();
    seed(&mut app);
    let origin = app.current_folder.clone();
    app.open_tag_view_for_tag("cat");
    wait_for_tag_result(&mut app);
    app.settings.grid_view_mode = GridViewMode::Details;
    app.set_details_sort_key(DetailsSortKey::AppliedAt);
    assert!(app.details_sort_key_visible(DetailsSortKey::AppliedAt));
    app.execute_tag_view();
    assert!(app.tag_view_pending.is_some());
    let return_route = app
        .top_level_grid_view
        .return_origin()
        .unwrap()
        .route
        .clone();
    let child_route = facet_navigation::FacetRoute::root(FacetScope::path(
        &origin.clone().unwrap().join("child"),
    ));
    assert_ne!(return_route, child_route);
    app.facet_navigation.reproject(child_route);
    app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    assert!(app.is_snapshot_active());
    let fixed = app.items.clone();
    let generation = app.items_generation;
    assert_eq!(
        app.snapshot.as_ref().unwrap().pre_snapshot_search_origin,
        origin
    );
    assert_eq!(
        app.top_level_grid_view.return_origin().unwrap().route,
        return_route
    );
    assert!(!app.tag_view.active);
    assert!(!app.items_are_tag_view);
    assert!(app.tag_view_pending.is_none());
    assert!(app.tag_view_applied_times.is_empty());
    assert_eq!(app.tag_view_applied_at_ms(0), None);
    assert!(!app.details_sort_key_visible(DetailsSortKey::AppliedAt));
    assert_eq!(app.settings.details_sort_key, DetailsSortKey::Toolbar);
    app.poll_tag_view();
    assert_eq!(app.items, fixed);
    assert_eq!(app.items_generation, generation);
    app.deactivate_snapshot();
    assert!(!app.items_are_tag_view);
    assert!(!app.details_sort_key_visible(DetailsSortKey::AppliedAt));
}
