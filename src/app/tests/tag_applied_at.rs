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

/// §1.476 reproduction: the tag bar searches on every edit, so an unfinished IME word
/// is adopted as a result list while the viewer is open.
#[test]
fn section1476_tag_bar_ime_composition_keeps_viewer_of_final_result() {
    let _input = crate::key_input::lock_test_input();
    let mut app = phase_c_support::setup_app();
    let (image, _) = seed(&mut app);
    image::RgbImage::new(2, 3).save(&image).unwrap();
    let mut db = crate::tags_db::TagsDb::open_at(&crate::data_dir::get().join("tags.db")).unwrap();
    db.set_item_tags(
        &crate::tags_db::item_key_for_path(&image),
        ["cat", "猫"],
        "test",
    )
    .unwrap();
    drop(db);
    app.open_tag_view_for_tag("cat");
    wait_for_tag_result(&mut app);
    let idx = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(image.as_path()))
        .unwrap();
    app.open_fullscreen(idx, HistoryTrigger::UserChosen);
    assert_eq!(app.fullscreen_idx, Some(idx));

    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    crate::ui_fonts::configure_fonts(&ctx);
    app.tag_view.focus_request = true;
    let mut trace = Vec::new();
    let frames: Vec<Vec<egui::Event>> = vec![
        vec![],
        vec![egui::Event::Key {
            key: egui::Key::A,
            physical_key: Some(egui::Key::A),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }],
        vec![
            egui::Event::Ime(egui::ImeEvent::Enabled),
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "ね".into(),
                active_range_chars: None,
            }),
        ],
        vec![egui::Event::Ime(egui::ImeEvent::Preedit {
            text: "ねこ".into(),
            active_range_chars: None,
        })],
        // The IME's Enter confirms composition, not the search.
        vec![tag_bar_key(egui::Key::Enter, true, egui::Modifiers::NONE)],
        vec![tag_bar_key(egui::Key::Enter, false, egui::Modifiers::NONE)],
        vec![egui::Event::Ime(egui::ImeEvent::Commit("猫".into()))],
    ];
    for events in frames {
        let output = ctx.run_pass(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                app.update_ime_state(ctx);
                ctx.root_ui(|root| app.render_tag_view_bar(root));
            },
        );
        let _ = crate::test_ui::headless_paint_output(output);
        assert!(app.tag_view_pending.is_none());
        assert_eq!(app.tag_view.last_executed, "#cat");
        assert_eq!(app.fullscreen_idx, Some(idx));
        trace.push((app.tag_view.query.clone(), app.fullscreen_idx));
    }
    tag_bar_frame(
        &mut app,
        &ctx,
        vec![tag_bar_key(egui::Key::Enter, true, egui::Modifiers::NONE)],
    );
    assert!(app.tag_view_pending.is_some());
    wait_for_tag_result(&mut app);
    assert_eq!(app.tag_view.last_executed, "猫", "{trace:?}");
    let open = app
        .fullscreen_idx
        .and_then(|idx| app.items.get(idx))
        .and_then(GridItem::drag_source_path);
    assert_eq!(open, Some(image.as_path()), "{trace:?}");
}

fn tag_bar_key(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    }
}

fn tag_bar_frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
    // No native keyboard sampling: the entire input batch is synthetic egui input.
    crate::key_input::set_test_frame(vec![]);
    let output = ctx.run_pass(
        egui::RawInput {
            events,
            ..Default::default()
        },
        |ctx| {
            app.update_ime_state(ctx);
            ctx.root_ui(|root| app.render_tag_view_bar(root));
        },
    );
    let _ = crate::test_ui::headless_paint_output(output);
}

fn tag_bar_context(app: &mut App) -> egui::Context {
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    crate::ui_fonts::configure_fonts(&ctx);
    app.tag_view.focus_request = true;
    tag_bar_frame(app, &ctx, vec![]);
    ctx
}

fn replace_tag_bar_text(app: &mut App, ctx: &egui::Context, text: &str) {
    app.tag_view.focus_request = true;
    tag_bar_frame(app, ctx, vec![]);
    tag_bar_frame(
        app,
        ctx,
        vec![tag_bar_key(egui::Key::A, true, egui::Modifiers::COMMAND)],
    );
    tag_bar_frame(
        app,
        ctx,
        vec![tag_bar_key(egui::Key::A, false, egui::Modifiers::COMMAND)],
    );
    let events = if text.is_empty() {
        vec![tag_bar_key(
            egui::Key::Backspace,
            true,
            egui::Modifiers::NONE,
        )]
    } else {
        vec![egui::Event::Text(text.into())]
    };
    tag_bar_frame(app, ctx, events);
    assert_eq!(app.tag_view.query, text);
}

fn tag_bar_viewer_fixture() -> (crate::app::AppTestEnvForTest, PathBuf) {
    let mut app = phase_c_support::setup_app();
    let (image, _) = seed(&mut app);
    image::RgbImage::new(2, 3).save(&image).unwrap();
    app.open_tag_view_for_tag("cat");
    wait_for_tag_result(&mut app);
    app.open_fullscreen(0, HistoryTrigger::UserChosen);
    assert!(app.projected_viewer_session_open());
    (app, image)
}

#[test]
fn section1476_tag_bar_typing_waits_for_enter_and_searches_once() {
    let _input = crate::key_input::lock_test_input();
    let (mut app, image) = tag_bar_viewer_fixture();
    let ctx = tag_bar_context(&mut app);
    let original = app.items.clone();
    let run = app.tag_view.run;
    replace_tag_bar_text(&mut app, &ctx, "#ca");
    assert!(app.tag_view_pending.is_none());
    assert_eq!(app.items, original);
    assert_eq!(app.tag_view.run, run);
    tag_bar_frame(
        &mut app,
        &ctx,
        vec![tag_bar_key(egui::Key::Enter, true, egui::Modifiers::NONE)],
    );
    assert!(app.tag_view_pending.is_some());
    assert_eq!(app.tag_view.run, run + 1);
    tag_bar_frame(&mut app, &ctx, vec![]);
    assert_eq!(app.tag_view.run, run + 1);
    wait_for_tag_result(&mut app);
    assert_eq!(
        app.items[app.fullscreen_idx.unwrap()].drag_source_path(),
        Some(image.as_path())
    );
    // Reconfirming an unchanged query must not start another worker.
    app.tag_view.focus_request = true;
    tag_bar_frame(
        &mut app,
        &ctx,
        vec![tag_bar_key(egui::Key::Enter, false, egui::Modifiers::NONE)],
    );
    tag_bar_frame(
        &mut app,
        &ctx,
        vec![tag_bar_key(egui::Key::Enter, true, egui::Modifiers::NONE)],
    );
    assert_eq!(app.tag_view.run, run + 1);
    assert!(app.tag_view_pending.is_none());
}

#[test]
fn section1476_tag_bar_enter_closes_viewer_excluded_from_result() {
    let _input = crate::key_input::lock_test_input();
    let (mut app, _) = tag_bar_viewer_fixture();
    let ctx = tag_bar_context(&mut app);
    replace_tag_bar_text(&mut app, &ctx, "#absent");
    assert!(app.tag_view_pending.is_none());
    assert!(app.projected_viewer_session_open());
    tag_bar_frame(
        &mut app,
        &ctx,
        vec![tag_bar_key(egui::Key::Enter, true, egui::Modifiers::NONE)],
    );
    wait_for_tag_result(&mut app);
    assert!(app.items.is_empty());
    assert!(!app.projected_viewer_session_open());
}

#[test]
fn section1476_tag_bar_empty_draft_installs_empty_results_only_on_enter() {
    let _input = crate::key_input::lock_test_input();
    let (mut app, _) = tag_bar_viewer_fixture();
    let ctx = tag_bar_context(&mut app);
    let original = app.items.clone();
    let saved = app.tag_view.saved_folder.clone();
    replace_tag_bar_text(&mut app, &ctx, "");
    assert!(app.tag_view_pending.is_none());
    assert!(app.items_are_tag_view);
    assert_eq!(app.items, original);
    assert!(app.projected_viewer_session_open());
    tag_bar_frame(
        &mut app,
        &ctx,
        vec![tag_bar_key(egui::Key::Enter, true, egui::Modifiers::NONE)],
    );
    assert!(app.tag_view_pending.is_some());
    wait_for_tag_result(&mut app);
    assert!(app.items_are_tag_view);
    assert!(app.items.is_empty());
    assert!(!app.projected_viewer_session_open());
    assert_eq!(app.tag_view.result_count, 0);
    assert_eq!(app.tag_view.saved_folder, saved);
    assert_eq!(app.address, "タグビュー");
    assert!(!app.sidecar_restore_active());
}

#[test]
fn section1476_tag_bar_without_viewer_searches_each_edit_but_not_on_close() {
    let _input = crate::key_input::lock_test_input();
    let (mut app, _) = tag_bar_viewer_fixture();
    let ctx = tag_bar_context(&mut app);
    replace_tag_bar_text(&mut app, &ctx, "#ca");
    let run = app.tag_view.run;
    app.close_fullscreen();
    assert!(!app.projected_viewer_session_open());
    tag_bar_frame(&mut app, &ctx, vec![]);
    assert_eq!(app.tag_view.run, run);
    assert!(app.tag_view_pending.is_none());
    // Focusing the field must still leave the draft unsearched.
    app.tag_view.focus_request = true;
    tag_bar_frame(&mut app, &ctx, vec![]);
    assert!(ctx.memory(|memory| memory.has_focus(App::tag_search_input_id())));
    assert_eq!(app.tag_view.run, run);
    // Each edit must differ from the executed #cat; typing "t" would restore
    // that same query and correctly skip a redundant search.
    for (offset, text) in ["x", "y"].into_iter().enumerate() {
        tag_bar_frame(&mut app, &ctx, vec![egui::Event::Text(text.into())]);
        assert!(app.tag_view_pending.is_some());
        assert_eq!(app.tag_view.run, run + offset as u64 + 1);
        wait_for_tag_result(&mut app);
    }
    assert_eq!(app.tag_view.last_executed, "#caxy");
}

#[test]
fn section1476_tag_bar_menu_and_kind_selection_search_immediately_while_viewing() {
    use egui_kittest::kittest::Queryable;
    let _input = crate::key_input::lock_test_input();
    let (mut app, image) = tag_bar_viewer_fixture();
    // Start from a different executed query so the menu selection must search.
    app.tag_view.query = "#ca".into();
    app.execute_tag_view();
    wait_for_tag_result(&mut app);
    app.tag_view.query = "#unfinished".into();
    let mut fonts_ready = false;
    let mut harness = crate::test_ui::builder()
        .with_size(egui::vec2(1093.0, 614.0))
        .build_ui_state(
            move |root, app: &mut crate::app::AppTestEnvForTest| {
                let ctx = root.ctx().clone();
                if !fonts_ready {
                    crate::ime_focus::install_ime_input_policy(&ctx);
                    crate::ui_fonts::configure_fonts(&ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                app.update_ime_state(&ctx);
                app.render_tag_view_bar(root);
            },
            app,
        );
    harness.run();
    let run = harness.state().tag_view.run;
    harness.get_by_label("一覧▼").click();
    harness.run();
    harness.get_by_label("#cat (2)").click();
    harness.run();
    assert_eq!(harness.state().tag_view.run, run + 1);
    assert!(harness.state().tag_view_pending.is_some());
    wait_for_tag_result(harness.state_mut());
    assert_eq!(harness.state().tag_view.last_executed, "#cat");
    // Picking the already executed tag again is the shipped way to search again.
    harness.get_by_label("一覧▼").click();
    harness.run();
    harness.get_by_label("#cat (2)").click();
    harness.run();
    assert_eq!(harness.state().tag_view.run, run + 2);
    wait_for_tag_result(harness.state_mut());
    harness.get_by_label("種類").click();
    harness.run();
    harness.get_by_label("画像").click();
    harness.run();
    assert_eq!(harness.state().tag_view.run, run + 3);
    assert!(harness.state().tag_view_pending.is_some());
    wait_for_tag_result(harness.state_mut());
    assert_eq!(
        harness.state().tag_view.last_executed_kind_filter,
        TagViewKindFilter::Image
    );
    assert_eq!(harness.state().items.len(), 1);
    assert_eq!(
        harness.state().items[harness.state().fullscreen_idx.unwrap()].drag_source_path(),
        Some(image.as_path())
    );
}

#[test]
fn section1476_tag_rename_merge_completion_requeries_results_but_preserves_book_items() {
    let mut app = phase_c_support::setup_app();
    let (_, folder) = seed(&mut app);
    app.open_tag_view_for_tag("cat");
    wait_for_tag_result(&mut app);
    let run = app.tag_view.run;
    let (tx, rx) = std::sync::mpsc::channel();
    app.tag_maintenance_rx = Some(rx);
    tx.send(Ok("renamed".into())).unwrap();
    app.poll_tag_maintenance_results();
    assert!(app.tag_maintenance_rx.is_none());
    assert!(app.tag_view_pending.is_some());
    assert_eq!(app.tag_view.run, run + 1);
    wait_for_tag_result(&mut app);

    app.tag_view.nav_stack.push(folder.clone());
    app.items_are_tag_view = false;
    let page = folder.join("page.jpg");
    app.items = vec![GridItem::Image(page)];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.image_metas = vec![None];
    let original = app.items.clone();
    let generation = app.items_generation;
    let (tx, rx) = std::sync::mpsc::channel();
    app.tag_maintenance_rx = Some(rx);
    tx.send(Ok("merged".into())).unwrap();
    app.poll_tag_maintenance_results();
    app.poll_tag_view();
    assert!(app.tag_maintenance_rx.is_none());
    assert!(app.tag_view_pending.is_none());
    assert_eq!(app.tag_view.run, run + 1);
    assert_eq!(app.items, original);
    assert_eq!(app.items_generation, generation);
    assert_eq!(app.tag_view.nav_stack, [folder]);
    app.tag_view_back();
    assert!(app.tag_view_pending.is_some());
    wait_for_tag_result(&mut app);
    assert!(app.items_are_tag_view);
    assert_eq!(app.items.len(), 2);
}

#[test]
fn section1476_automatic_refresh_searches_executed_word_and_keeps_draft_and_viewer() {
    let (mut app, image) = tag_bar_viewer_fixture();
    // An unsubmitted word stays in the field while the F12-linked reader is open.
    app.tag_view.query = "#absent".into();
    for refresh in ["rename", "cleanup"] {
        let run = app.tag_view.run;
        if refresh == "rename" {
            let (tx, rx) = std::sync::mpsc::channel();
            app.tag_maintenance_rx = Some(rx);
            tx.send(Ok("renamed".into())).unwrap();
            app.poll_tag_maintenance_results();
        } else {
            app.reload_open_metadata_views_after_cleanup();
        }
        assert_eq!(app.tag_view.run, run + 1, "{refresh}");
        assert_eq!(app.tag_view.last_executed, "#cat", "{refresh}");
        assert_eq!(app.tag_view.query, "#absent", "{refresh}");
        wait_for_tag_result(&mut app);
        let open = app
            .fullscreen_idx
            .and_then(|idx| app.items.get(idx))
            .and_then(GridItem::drag_source_path);
        assert_eq!(open, Some(image.as_path()), "{refresh}");
        assert_eq!(app.address, "タグビュー: #cat", "{refresh}");
    }
}

#[test]
fn section1476_adopted_result_names_its_own_word_not_a_later_draft() {
    let (mut app, _) = tag_bar_viewer_fixture();
    app.set_tag_view_sort(TagViewSort::AppliedAtDesc);
    assert!(app.tag_view_pending.is_some());
    // The draft changes while the executed search is still in flight.
    app.tag_view.query = "#absent".into();
    wait_for_tag_result(&mut app);
    assert!(app.fullscreen_idx.is_some());
    assert_eq!(app.tag_view.last_executed, "#cat");
    assert_eq!(app.address, "タグビュー: #cat");
}
