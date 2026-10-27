#![cfg(test)]

//! Headless lifecycle checks. The name-search transport is never called: completed datasets
//! enter the same Search prepare/adoption owner used by production.
use super::*;
use crate::everything_results::{EverythingDataset, EverythingPreparedRow, EverythingStats};
use crate::settings::{ListingSortMetadata, SortOrder};

pub(crate) fn dataset(items: Vec<GridItem>) -> Arc<EverythingDataset> {
    let count = items.len();
    Arc::new(EverythingDataset {
        rows: items
            .into_iter()
            .enumerate()
            .map(|(index, item)| EverythingPreparedRow {
                physical_key: crate::path_key::physical_path_spelling(
                    item.drag_source_path().unwrap(),
                )
                .unwrap(),
                item: item.clone(),
                image_meta: Some((
                    index as i64 + 10,
                    if matches!(item, GridItem::Folder(_)) {
                        0
                    } else {
                        index as i64 + 20
                    },
                )),
                sort_meta: ListingSortMetadata::new(
                    index as i64 + 10,
                    (!matches!(item, GridItem::Folder(_))).then_some(index as i64 + 20),
                ),
            })
            .collect(),
        stats: EverythingStats {
            total_items: count as u32,
            raw_returned: count,
            ..Default::default()
        },
    })
}

fn accepted_dataset(app: &App) -> &Arc<EverythingDataset> {
    let SearchAccepted::Everything { dataset, .. } = &app.global_search.accepted else {
        panic!("Everything dataset was not accepted");
    };
    dataset
}

pub(crate) fn start_fake_run(
    app: &mut App,
    ctx: &egui::Context,
    query: &str,
) -> crate::everything_ipc::CancelHandle {
    if !app.global_search.active {
        app.open_global_search();
    }
    app.switch_search_source(SearchSource::Everything);
    app.global_search.pending = None;
    app.global_search.page_edit_prepare = None;
    app.global_search.run_sequence = app.global_search.run_sequence.wrapping_add(1);
    app.global_search.everything_draft = query.into();
    app.global_search.visibility = app.settings.file_type_visibility.clone();
    app.global_search.page_edit_prepare = Some(
        SearchPageEditPrepare::spawn(
            ctx,
            crate::app::page_edit_snapshot::PageEditAvailability::for_app(app),
            app.settings.file_type_visibility.clone(),
            app.settings.thumbnail_source_discovery_snapshot(),
            SearchMetadataResources::for_app(app),
        )
        .unwrap(),
    );
    let cancel = crate::everything_ipc::CancelHandle::new().unwrap();
    app.global_search.pending = Some(SearchProducer::Everything {
        intent: EverythingIntent::Query,
        request_run: app.global_search.run_sequence,
        local_lease: None,
        cancel: cancel.clone(),
        submitted: SubmittedEverything {
            query: query.into(),
            run: app.global_search.run_sequence,
        },
        context: app.virtual_list_context_id(),
        generation: app.items_generation,
        policy: Box::new(app.settings.thumbnail_source_discovery_snapshot()),
    });
    cancel
}

fn queue(app: &mut App, dataset: Arc<EverythingDataset>) {
    let mut prepare = app.global_search.page_edit_prepare.take().unwrap();
    app.queue_everything_view(&mut prepare, dataset, None);
    app.global_search.page_edit_prepare = Some(prepare);
}

fn poll_until(app: &mut App, ctx: &egui::Context, reason: &str, ready: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready(app) {
        app.poll_search_page_edit_prepare(ctx);
        assert!(Instant::now() < deadline, "{reason}");
        std::thread::yield_now();
    }
}

pub(crate) fn adopt(
    app: &mut App,
    ctx: &egui::Context,
    query: &str,
    dataset: Arc<EverythingDataset>,
) {
    start_fake_run(app, ctx, query);
    queue(app, Arc::clone(&dataset));
    poll_until(app, ctx, "completed dataset did not adopt", |app| {
        app.global_search.pending.is_none()
            && matches!(&app.global_search.accepted, SearchAccepted::Everything { dataset: accepted, .. } if Arc::ptr_eq(accepted, &dataset))
    });
}

/// A live producer owns channels and cancellation, but its worker is deliberately not finished.
/// This lets cancel exercise the actual Drop boundaries without transport, timeouts, or joins.
fn stalled_prepare() -> (
    SearchPageEditPrepare,
    mpsc::Receiver<SearchPrepareCommand>,
    mpsc::Sender<SearchPrepareOutput>,
) {
    let (tx, command_rx) = mpsc::channel();
    let (output_tx, rx) = mpsc::channel();
    (
        SearchPageEditPrepare {
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            next_sequence: 0,
            raw_batch_sequence: 0,
            rated_batch_sequence: 0,
            in_flight: None,
            desired: None,
            hold_ready_for_test: false,
            held_ready_for_test: None,
            metrics: Default::default(),
        },
        command_rx,
        output_tx,
    )
}

#[test]
fn section1431_drive_list_search_initializes_thumbnail_owner_and_restores_icons() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    app.folder_pin_map.clear();
    app.enter_drive_list(None);
    let drives = app.items.clone();
    assert!(app.reload_queue.is_none());
    assert!(app.current_color_cache_map.is_none());
    assert!(
        app.thumbnails
            .iter()
            .all(|state| matches!(state, ThumbnailState::Failed))
    );
    let root = app.tmp.path().join("results");
    std::fs::create_dir_all(root.join("folder")).unwrap();
    let result = dataset(vec![
        GridItem::Image(root.join("image.jpg")),
        GridItem::ZipFile(root.join("book.zip")),
        GridItem::Folder(root.join("folder")),
    ]);
    #[cfg(windows)]
    let sibling = app.build_window_context_for_test(1431, |mounted| {
        mounted.load_folder(root.clone());
        mounted.finish_page_identity_preparation_for_test();
    });
    #[cfg(windows)]
    let (sibling_cancel, sibling_queue, sibling_generation, sibling_ticket) = app
        .with_window_context_for_test(sibling, |mounted| {
            (
                Arc::clone(&mounted.cancel_token),
                Arc::clone(mounted.reload_queue.as_ref().unwrap()),
                mounted.items_generation,
                mounted.pdf_render_epoch.snapshot(),
            )
        });
    adopt(&mut app, &ctx, "images and books", result);
    assert!(
        app.reload_queue.is_some(),
        "accepted search needs regular workers"
    );
    assert!(app.heavy_io_queue.is_some());
    assert!(app.current_color_cache_map.is_some());
    assert!(app.current_color_catalog.is_some());
    let pages = (0..app.items.len()).collect::<Vec<_>>();
    app.ensure_navigation_target_thumbnail_requests(&ctx, &pages);
    assert!(pages.iter().all(|index| app.requested.contains_key(index)));
    let cancel = Arc::clone(&app.cancel_token);
    app.close_global_search();
    app.finish_history_restore_preparation_for_test();
    assert!(!app.global_search.active);
    assert!(app.items_are_drive_list);
    assert_eq!(app.items, drives);
    assert!(app.folder_pin_map.is_empty());
    assert!(
        app.thumbnails
            .iter()
            .all(|state| matches!(state, ThumbnailState::Failed))
    );
    assert!(app.reload_queue.is_none());
    assert!(app.heavy_io_queue.is_none());
    assert!(cancel.load(Ordering::Relaxed));
    #[cfg(windows)]
    app.with_window_context_for_test(sibling, |mounted| {
        assert!(Arc::ptr_eq(&mounted.cancel_token, &sibling_cancel));
        assert!(!sibling_cancel.load(Ordering::Relaxed));
        assert!(Arc::ptr_eq(
            mounted.reload_queue.as_ref().unwrap(),
            &sibling_queue
        ));
        assert_eq!(mounted.items_generation, sibling_generation);
        assert!(sibling_ticket.is_current());
    });
}

#[test]
fn section1431_metadata_search_from_drive_list_initializes_once_and_restores_icons() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    app.folder_pin_map.clear();
    app.enter_drive_list(None);
    let drives = app.items.clone();
    let image = app.tmp.path().join("metadata.jpg");
    std::fs::write(&image, b"image").unwrap();
    crate::global_search_ui::tests::g1_publish_search_hits(&mut app, &ctx, &[image]);
    let queue = Arc::clone(
        app.reload_queue
            .as_ref()
            .expect("Ctrl+G needs regular workers"),
    );
    let cancel = Arc::clone(&app.cancel_token);
    app.ensure_navigation_target_thumbnail_requests(&ctx, &[0]);
    assert!(app.requested.contains_key(&0));
    app.rebuild_items_from_global_search();
    poll_until(
        &mut app,
        &ctx,
        "metadata reprojection did not settle",
        |app| {
            let prepare = app.global_search.page_edit_prepare.as_ref().unwrap();
            prepare.in_flight.is_none() && prepare.desired.is_none()
        },
    );
    assert!(Arc::ptr_eq(app.reload_queue.as_ref().unwrap(), &queue));
    assert!(Arc::ptr_eq(&app.cancel_token, &cancel));
    assert!(!cancel.load(Ordering::Relaxed));
    app.close_global_search();
    app.finish_history_restore_preparation_for_test();
    assert!(app.items_are_drive_list);
    assert_eq!(app.items, drives);
    assert!(
        app.thumbnails
            .iter()
            .all(|state| matches!(state, ThumbnailState::Failed))
    );
    assert!(app.reload_queue.is_none());
    assert!(cancel.load(Ordering::Relaxed));
}

#[test]
fn section1431_cancelled_prepare_does_not_install_thumbnail_workers() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    app.folder_pin_map.clear();
    app.enter_drive_list(None);
    let drives = app.items.clone();
    start_fake_run(&mut app, &ctx, "image");
    app.hold_next_search_ready_for_test();
    let image = GridItem::Image(app.tmp.path().join("not-adopted.jpg"));
    queue(&mut app, dataset(vec![image]));
    poll_until(&mut app, &ctx, "prepared result was not held", |app| {
        app.search_ready_is_held_for_test()
    });
    assert!(app.reload_queue.is_none());
    assert!(app.current_color_cache_map.is_none());
    app.close_global_search();
    app.finish_history_restore_preparation_for_test();
    assert!(app.items_are_drive_list);
    assert_eq!(app.items, drives);
    assert!(app.reload_queue.is_none());
    assert!(app.current_color_cache_map.is_none());
}

#[test]
fn completed_dataset_publishes_rows_stamps_identity_and_source_at_one_ready_boundary() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let a = GridItem::Image(app.tmp.path().join("a.jpg"));
    let b = GridItem::Image(app.tmp.path().join("b.jpg"));
    let result = dataset(vec![a.clone(), b.clone()]);
    start_fake_run(&mut app, &ctx, "ext:jpg");
    app.hold_next_search_ready_for_test();
    let old_items = app.items.clone();
    queue(&mut app, Arc::clone(&result));
    poll_until(
        &mut app,
        &ctx,
        "Ready not held",
        App::search_ready_is_held_for_test,
    );
    assert_eq!(app.items, old_items);
    assert!(matches!(
        app.global_search.accepted,
        SearchAccepted::Metadata { .. }
    ));
    assert!(app.everything_search_progress_visible());
    assert_eq!(
        app.everything_search_entry_availability(),
        EverythingSearchEntryAvailability::OtherOperation
    );
    assert!(!app.everything_search_entry_allowed());
    app.release_search_ready_for_test();
    poll_until(&mut app, &ctx, "Ready did not adopt", |app| {
        app.global_search.pending.is_none()
    });
    assert_eq!(app.items, vec![a, b]);
    assert_eq!(app.image_metas, vec![Some((10, 20)), Some((11, 21))]);
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    assert!(app.items_are_global_search_view);
    assert!(!app.everything_search_progress_visible());
    assert_eq!(app.global_search.last_executed, "ext:jpg");
    assert_eq!(
        app.projection_recipe,
        crate::app::book_materialization::ProjectionRecipe::ExtractedResult(
            crate::app::book_materialization::ExtractedSurface::Search(
                crate::app::top_level_grid_view::TopLevelSearchView::Global,
            ),
        ),
        "prepared Everything rows use the G1 extracted projection at adoption",
    );
    for (index, item) in app.items.iter().enumerate() {
        assert_eq!(
            app.page_identity_index
                .resolve(&crate::book_resume_db::ReadingPosition {
                    page: 999,
                    identity: crate::book_resume_db::PageIdentity::for_item(item, None),
                }),
            crate::app::file_type_resume::PositionResolution::Restored(index),
        );
    }
}

#[test]
fn stale_prepared_pins_and_tags_retry_before_any_metadata_is_published() {
    use crate::folder_thumb_pins::{FolderPinSource, FolderThumbPinDb};
    let _epochs = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    app.settings.sort_order = SortOrder::FileName;
    app.settings.write_rating_to_xmp = false;
    let zip = app.tmp.path().join("book.zip");
    let rar = app.tmp.path().join("converted.rar");
    let image = app.tmp.path().join("image.jpg");
    let image_key = crate::tags_db::item_key_for_path(&image);
    let zip_key = crate::path_key::normalize_keep_drive(&zip);
    let rar_key = crate::path_key::normalize_keep_drive(&rar);
    let pins = Arc::new(FolderThumbPinDb::open_at(&app.tmp.path().join("pins.db")).unwrap());
    let old_pin = FolderPinSource::ZipEntry {
        zip_rel: String::new(),
        entry: "old-cover.jpg".into(),
    };
    let fresh_pin = FolderPinSource::ZipEntry {
        zip_rel: String::new(),
        entry: "fresh-cover.jpg".into(),
    };
    pins.set(&zip, &old_pin).unwrap();
    let mut tags = crate::tags_db::TagsDb::open_at(&app.tmp.path().join("tags.db")).unwrap();
    tags.set_item_tags(&image_key, ["before"], crate::tags_db::source::EDIT)
        .unwrap();
    app.folder_thumb_pin_db = Some(Arc::clone(&pins));
    app.tags_db = Some(tags);
    let result = dataset(vec![
        GridItem::ZipFile(zip),
        GridItem::ConvertibleArchive {
            path: rar,
            format: crate::archive_converter::ArchiveFormat::Rar,
        },
        GridItem::Image(image),
    ]);
    start_fake_run(&mut app, &ctx, "fake metadata");
    app.hold_next_search_ready_for_test();
    crate::app::RATING_INSTALL_UI_TAG_PREWARMS.with(|calls| calls.set(0));
    let old_rows = app.items.clone();
    let old_generation = app.items_generation;
    let old_visible_pins = app.folder_pin_map.clone();
    let old_visible_tags = app.tags_cache.clone();
    queue(&mut app, Arc::clone(&result));
    poll_until(
        &mut app,
        &ctx,
        "metadata Ready not held",
        App::search_ready_is_held_for_test,
    );
    {
        let Some(SearchPrepareOutput::Ready(_, prepared)) = &app
            .global_search
            .page_edit_prepare
            .as_ref()
            .unwrap()
            .held_ready_for_test
        else {
            panic!("expected held metadata Ready");
        };
        let SearchMetadataInstall::Prepared(metadata) = &prepared.metadata else {
            panic!("Everything metadata must be worker prepared");
        };
        assert_eq!(metadata.pins[&zip_key], old_pin);
        assert_eq!(metadata.tags[&image_key], vec!["#before"]);
    }
    // These ordinary edits occur after the worker has finished its immutable payload.
    pins.set(
        result.rows[0].item.file_operation_path().unwrap(),
        &fresh_pin,
    )
    .unwrap();
    app.tags_db
        .as_mut()
        .unwrap()
        .set_item_tags(&image_key, ["after"], crate::tags_db::source::EDIT)
        .unwrap();
    app.release_search_ready_for_test();
    app.poll_search_page_edit_prepare(&ctx);
    assert_eq!(app.search_prepare_metrics_for_test().stale_rejections, 1);
    assert_eq!(app.items_generation, old_generation);
    assert_eq!(app.items, old_rows);
    assert_eq!(app.folder_pin_map, old_visible_pins);
    assert_eq!(app.tags_cache, old_visible_tags);
    assert!(app.everything_search_progress_visible());
    assert!(matches!(
        app.global_search.accepted,
        SearchAccepted::Metadata { .. }
    ));
    crate::app::RATING_INSTALL_UI_TAG_PREWARMS.with(|calls| assert_eq!(calls.get(), 0));
    app.hold_next_search_ready_for_test();
    poll_until(
        &mut app,
        &ctx,
        "fresh metadata retry Ready not held",
        App::search_ready_is_held_for_test,
    );
    assert_eq!(app.items_generation, old_generation);
    app.release_search_ready_for_test();
    poll_until(
        &mut app,
        &ctx,
        "fresh metadata retry did not adopt",
        |app| app.global_search.pending.is_none(),
    );
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    assert_eq!(app.folder_pin_map[&zip_key], fresh_pin);
    assert_eq!(app.tags_cache[&image_key], vec!["#after"]);
    assert_eq!(
        app.converted_archive_cache_paths[&rar_key],
        crate::app::ConvertedArchiveSourceState::Pending
    );
    assert_eq!(app.items.len(), 3);
    crate::app::RATING_INSTALL_UI_TAG_PREWARMS.with(|calls| assert_eq!(calls.get(), 0));
}

#[test]
fn cancel_drops_both_receivers_without_waiting_or_replacing_accepted_rows() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let result = dataset(vec![GridItem::Image(app.tmp.path().join("old.jpg"))]);
    adopt(&mut app, &ctx, "old", Arc::clone(&result));
    let old_rows = app.items.clone();
    let old_generation = app.items_generation;
    let request_cancel = start_fake_run(&mut app, &ctx, "new");
    let (prepare, command_rx, output_tx) = stalled_prepare();
    let prepare_cancel = Arc::clone(&prepare.cancel);
    app.global_search.page_edit_prepare = Some(prepare);
    assert!(app.common_modal_dialog_open());
    app.cancel_everything_search();
    assert!(request_cancel.is_cancelled());
    assert!(prepare_cancel.load(Ordering::Relaxed));
    assert!(
        matches!(
            command_rx.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ),
        "prepare sender must be retired"
    );
    assert!(
        output_tx
            .send(SearchPrepareOutput::EverythingFailed("late".into()))
            .is_err(),
        "late worker completion must have no receiver"
    );
    assert!(!app.everything_search_progress_visible());
    assert_eq!(app.items, old_rows);
    assert_eq!(app.items_generation, old_generation);
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    assert_eq!(app.global_search.last_executed, "old");
}

#[test]
fn failed_new_query_keeps_the_old_accepted_source_and_dataset() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let result = dataset(vec![GridItem::Image(app.tmp.path().join("old.jpg"))]);
    adopt(&mut app, &ctx, "old", Arc::clone(&result));
    let rows = app.items.clone();
    let cancel = start_fake_run(&mut app, &ctx, "new");
    let (prepare, _command_rx, output_tx) = stalled_prepare();
    app.global_search.page_edit_prepare = Some(prepare);
    output_tx
        .send(SearchPrepareOutput::EverythingFailed("not running".into()))
        .unwrap();
    app.poll_search_page_edit_prepare(&ctx);
    assert_eq!(app.items, rows);
    assert_eq!(app.global_search.everything_draft, "new");
    assert_eq!(app.global_search.last_executed, "old");
    assert_eq!(
        app.global_search.reject_message.as_deref(),
        Some("not running")
    );
    assert!(cancel.is_cancelled());
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
}

#[test]
fn source_a_b_a_keeps_one_original_return_owner_and_old_accepted_result() {
    use crate::app::top_level_grid_view::TopLevelGridRestore;
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let folder = app.tmp.path().join("origin");
    std::fs::create_dir(&folder).unwrap();
    app.current_folder = Some(folder.clone());
    let result = dataset(vec![GridItem::Image(folder.join("hit.jpg"))]);
    adopt(&mut app, &ctx, "hit", Arc::clone(&result));
    let route = app
        .top_level_grid_view
        .return_origin()
        .unwrap()
        .route
        .clone();
    let saved = app.global_search.saved_folder.clone();
    let cancel = start_fake_run(&mut app, &ctx, "pending");
    app.switch_search_source(SearchSource::Metadata);
    assert!(cancel.is_cancelled());
    app.switch_search_source(SearchSource::Everything);
    assert_eq!(app.global_search.saved_folder, saved);
    let origin = app.top_level_grid_view.return_origin().unwrap();
    assert!(matches!(&origin.restore, TopLevelGridRestore::Folder(path) if path == &folder));
    assert_eq!(origin.route, route);
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    assert_eq!(app.items, vec![GridItem::Image(folder.join("hit.jpg"))]);
    let origin = app.dismiss_global_search_with_origin();
    assert!(matches!(origin.restore, TopLevelGridRestore::Folder(path) if path == folder));
    assert!(app.top_level_grid_view.return_origin().is_none());
    assert!(!app.global_search.active);
}

#[test]
fn same_query_old_ready_cannot_cross_a_new_run_even_with_the_same_sequence() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let accepted = dataset(vec![GridItem::Image(app.tmp.path().join("accepted.jpg"))]);
    adopt(&mut app, &ctx, "same", Arc::clone(&accepted));
    start_fake_run(&mut app, &ctx, "same");
    app.hold_next_search_ready_for_test();
    let obsolete = dataset(vec![GridItem::Image(app.tmp.path().join("obsolete.jpg"))]);
    queue(&mut app, obsolete);
    poll_until(
        &mut app,
        &ctx,
        "old Ready not held",
        App::search_ready_is_held_for_test,
    );
    let late = app
        .global_search
        .page_edit_prepare
        .as_mut()
        .unwrap()
        .held_ready_for_test
        .take()
        .unwrap();
    let SearchPrepareOutput::Ready(old_wish, _) = &late else {
        panic!("expected Ready");
    };
    let old_run = old_wish.run_sequence;
    let sequence = old_wish.sequence;
    let old_rows = app.items.clone();
    start_fake_run(&mut app, &ctx, "same");
    assert_ne!(app.global_search.run_sequence, old_run);
    let (prepare, _command_rx, output_tx) = stalled_prepare();
    app.global_search.page_edit_prepare = Some(prepare);
    let fresh = dataset(vec![GridItem::Image(app.tmp.path().join("fresh.jpg"))]);
    queue(&mut app, fresh);
    let prepare = app.global_search.page_edit_prepare.as_mut().unwrap();
    prepare.in_flight = Some(sequence);
    prepare.desired.as_mut().unwrap().sequence = sequence;
    output_tx.send(late).unwrap();
    app.poll_search_page_edit_prepare(&ctx);
    assert_eq!(app.items, old_rows);
    assert!(Arc::ptr_eq(accepted_dataset(&app), &accepted));
    assert_eq!(app.search_prepare_metrics_for_test().stale_rejections, 1);
    assert!(app.everything_search_progress_visible());
    app.cancel_everything_search();
}

#[test]
fn editing_draft_does_not_invalidate_sorting_of_the_submitted_dataset() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    app.settings.sort_order = SortOrder::FileName;
    let a = GridItem::Image(app.tmp.path().join("a.jpg"));
    let b = GridItem::Image(app.tmp.path().join("b.jpg"));
    let result = dataset(vec![b.clone(), a.clone()]);
    adopt(&mut app, &ctx, "submitted", Arc::clone(&result));
    assert_eq!(app.items, vec![a.clone(), b.clone()]);
    app.selected = Some(0);
    app.grid_click_selection_anchor = Some(crate::app::GridClickSelectionAnchor::new(
        0,
        app.items_generation,
    ));
    app.global_search.everything_draft = "draft only".into();
    app.settings.sort_order = SortOrder::FileNameDesc;
    app.hold_next_search_ready_for_test();
    app.rebuild_items_from_global_search();
    poll_until(
        &mut app,
        &ctx,
        "sort Ready not held",
        App::search_ready_is_held_for_test,
    );
    let before = app.items_generation;
    app.release_search_ready_for_test();
    poll_until(&mut app, &ctx, "sort Ready did not adopt", |app| {
        app.items_generation != before
    });
    assert_eq!(app.items, vec![b, a]);
    assert_eq!(app.selected, Some(1));
    assert_eq!(
        app.grid_click_selection_anchor
            .and_then(|anchor| anchor.index_for_generation(app.items_generation)),
        Some(1)
    );
    assert_eq!(app.image_metas, vec![Some((10, 20)), Some((11, 21))]);
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    assert_eq!(app.global_search.last_executed, "submitted");
    assert_eq!(app.global_search.everything_draft, "draft only");
}

#[test]
fn new_same_query_dataset_does_not_transfer_the_old_membership_neighbor() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    app.settings.sort_order = SortOrder::FileName;
    let old = dataset(
        ["a.jpg", "b.jpg", "c.jpg"]
            .into_iter()
            .map(|name| GridItem::Image(app.tmp.path().join("old").join(name)))
            .collect(),
    );
    adopt(&mut app, &ctx, "same query", old);
    app.selected = Some(2);
    app.grid_click_selection_anchor = Some(crate::app::GridClickSelectionAnchor::new(
        2,
        app.items_generation,
    ));
    let fresh = dataset(
        ["a.jpg", "b.jpg", "c.jpg"]
            .into_iter()
            .map(|name| GridItem::Image(app.tmp.path().join("fresh").join(name)))
            .collect(),
    );
    adopt(&mut app, &ctx, "same query", Arc::clone(&fresh));
    assert!(Arc::ptr_eq(accepted_dataset(&app), &fresh));
    // A new source uses ordinary first-visible selection and a fresh range anchor,
    // rather than the old list's disappeared anchor ordinal. Same physical identities
    // still use the shared swap helper.
    assert_eq!(app.selected, Some(0));
    assert_eq!(
        app.grid_click_selection_anchor
            .and_then(|anchor| anchor.index_for_generation(app.items_generation)),
        Some(0)
    );
}

#[test]
fn same_dataset_facet_reprojection_retains_selected_identity() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    app.settings.sort_order = SortOrder::FileName;
    let a = GridItem::Image(app.tmp.path().join("a.jpg"));
    let selected = GridItem::Image(app.tmp.path().join("b.jpg"));
    let book = GridItem::ZipFile(app.tmp.path().join("c.zip"));
    let result = dataset(vec![a, selected.clone(), book]);
    adopt(&mut app, &ctx, "submitted", Arc::clone(&result));
    app.selected = Some(1);
    app.grid_click_selection_anchor = Some(crate::app::GridClickSelectionAnchor::new(
        1,
        app.items_generation,
    ));
    app.settings
        .facet_filter
        .kinds
        .insert(crate::settings::FacetItemKind::Image);
    app.hold_next_search_ready_for_test();
    app.rebuild_items_from_global_search();
    poll_until(
        &mut app,
        &ctx,
        "facet Ready not held",
        App::search_ready_is_held_for_test,
    );
    let before = app.items_generation;
    app.release_search_ready_for_test();
    poll_until(&mut app, &ctx, "facet Ready did not adopt", |app| {
        app.items_generation != before
    });
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    assert_eq!(app.items[app.selected.unwrap()], selected);
    assert_eq!(app.visible_indices, vec![0, 1]);
    assert_eq!(
        app.grid_click_selection_anchor
            .and_then(|anchor| anchor.index_for_generation(app.items_generation)),
        Some(app.selected.unwrap())
    );
}

#[test]
fn physical_open_acceptance_keeps_logical_archive_identity_and_returns_exact_selection() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let logical = app.tmp.path().join("book.rar");
    let other = app.tmp.path().join("a.jpg");
    let cache = app.tmp.path().join("conversion-cache.zip");
    let archive = GridItem::ConvertibleArchive {
        path: logical.clone(),
        format: crate::archive_converter::ArchiveFormat::Rar,
    };
    let result = dataset(vec![archive.clone(), GridItem::Image(other)]);
    adopt(&mut app, &ctx, "book", Arc::clone(&result));
    assert!(!app.everything_has_physical_child());
    // Merely displaying/canceling conversion has no accepted navigation callback.
    assert!(matches!(
        &app.global_search.accepted,
        SearchAccepted::Everything {
            position: EverythingPosition::Root,
            ..
        }
    ));
    assert!(app.accept_everything_physical_child(&logical));
    assert!(
        matches!(&app.global_search.accepted, SearchAccepted::Everything {
        position: EverythingPosition::PhysicalChild { logical_source, return_anchor, .. }, ..
    } if logical_source == &logical && return_anchor == &logical)
    );
    app.current_folder = Some(cache.clone());
    app.projection_recipe = crate::app::book_materialization::ProjectionRecipe::Book(
        crate::app::book_materialization::BookScope::Archive {
            logical_root: logical.clone(),
            prefix: Vec::new(),
        },
    );
    app.items = vec![GridItem::ZipImage {
        zip_path: cache,
        entry_name: "page.jpg".into(),
    }];
    app.image_metas = vec![Some((99, 99))];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.items_generation = app.items_generation.wrapping_add(1);
    app.selected = Some(0);
    app.hold_next_search_ready_for_test();
    assert!(app.return_from_everything_child(EverythingReturn::Parent));
    poll_until(
        &mut app,
        &ctx,
        "return Ready not held",
        App::search_ready_is_held_for_test,
    );
    assert!(
        app.everything_has_physical_child(),
        "return is not published before Ready"
    );
    assert!(matches!(app.items[0], GridItem::ZipImage { .. }));
    app.release_search_ready_for_test();
    poll_until(&mut app, &ctx, "return did not adopt", |app| {
        !app.everything_has_physical_child()
    });
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    assert_eq!(app.items[app.selected.unwrap()], archive);
    assert!(app.scroll_to_selected);
    assert_eq!(app.global_search.last_executed, "book");
    assert_eq!(
        app.projection_recipe,
        crate::app::book_materialization::ProjectionRecipe::ExtractedResult(
            crate::app::book_materialization::ExtractedSurface::Search(
                crate::app::top_level_grid_view::TopLevelSearchView::Global,
            ),
        ),
        "return from an Everything book child restores the extracted recipe",
    );
}

#[test]
fn zip_prefix_parent_stays_in_the_physical_child_before_returning_to_results() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    // A real accepted book open keeps Search's originating navigation location. Merely setting
    // current_folder after a Search with Unavailable origin cannot create that return owner.
    app.current_folder = Some(app.tmp.path().to_path_buf());
    let zip = app.tmp.path().join("book.zip");
    let result = dataset(vec![GridItem::ZipFile(zip.clone())]);
    adopt(&mut app, &ctx, "book", Arc::clone(&result));
    assert!(app.accept_everything_physical_child(&zip));
    let tree = Arc::new(crate::zip_tree::ZipTree::build(
        zip.clone(),
        ["root.jpg", "chapter/page.jpg"]
            .map(|name| crate::zip_loader::ZipImageEntry {
                entry_name: name.into(),
                uncompressed_size: 1,
                mtime: 1,
                container_ancestry: Vec::new(),
            })
            .to_vec(),
    ));
    let mut nav = crate::zip_tree::ZipNavState::new(tree);
    nav.enter("chapter/");
    assert!(!nav.at_root());
    app.current_folder = Some(zip);
    app.zip_nav = Some(nav);
    assert!(app.return_from_everything_child(EverythingReturn::Parent));
    assert!(app.zip_nav.as_ref().unwrap().at_root());
    assert!(app.everything_has_physical_child());
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    assert!(app.items.iter().any(
        |item| matches!(item, GridItem::ZipDir { dir_prefix, .. } if dir_prefix == "chapter/")
    ));
}

fn assert_return_only(app: &App) {
    assert!(
        matches!(
            app.global_search.pending,
            Some(SearchProducer::Everything {
                intent: EverythingIntent::Return,
                ..
            })
        ),
        "retained return must never start an IPC Query"
    );
}

#[test]
fn folder_rows_share_size_sort_rating_mask_and_kind_tag_facets() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let folder = GridItem::Folder(app.tmp.path().join("name.zip"));
    let image = GridItem::Image(app.tmp.path().join("image.jpg"));
    let result = dataset(vec![folder.clone(), image.clone()]);
    let ratings = HashMap::from([(search_item_rating_key(&folder).unwrap(), 4)]);
    let mask = [false, false, false, false, true, false];
    let (items, metas) = project_dataset(
        &result,
        SortOrder::SizeAsc,
        &ratings,
        &mask,
        &Default::default(),
    );
    assert_eq!(
        items,
        vec![folder.clone()],
        "rating mask must include folder ratings"
    );
    assert_eq!(metas, vec![Some((10, 0))]);
    for sort in [SortOrder::SizeAsc, SortOrder::SizeDesc] {
        let (items, _) = project_dataset(&result, sort, &ratings, &[true; 6], &Default::default());
        assert_eq!(
            items,
            vec![image.clone(), folder.clone()],
            "folder size is unknown, not zero bytes"
        );
    }
    let mut tags = crate::tags_db::TagsDb::open_at(&app.tmp.path().join("folder-tags.db")).unwrap();
    let key = crate::tags_db::item_key_for_path(folder.drag_source_path().unwrap());
    tags.set_item_tags(&key, ["folder-tag"], crate::tags_db::source::EDIT)
        .unwrap();
    app.tags_db = Some(tags);
    app.settings
        .facet_filter
        .kinds
        .insert(crate::settings::FacetItemKind::Folder);
    app.settings.facet_filter.tags.insert("folder-tag".into());
    adopt(&mut app, &ctx, "mixed", Arc::clone(&result));
    assert_eq!(
        app.items.len(),
        2,
        "facets must not replace the retained dataset"
    );
    assert_eq!(app.visible_indices.len(), 1);
    assert_eq!(app.items[app.visible_indices[0]], folder);
    assert_eq!(app.tags_cache[&key], vec!["#folder-tag"]);
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
}

#[test]
fn folder_activation_parent_chain_gamepad_return_research_and_close_keep_one_owner() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let origin = app.tmp.path().join("origin");
    let folder = app.tmp.path().join("hit");
    let sub = folder.join("sub");
    std::fs::create_dir(&origin).unwrap();
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(origin.join("original.jpg"), b"image").unwrap();
    app.load_folder(origin.clone());
    app.finish_page_identity_preparation_for_test();
    let row = GridItem::Folder(folder.clone());
    let result = dataset(vec![
        row.clone(),
        GridItem::Image(app.tmp.path().join("other.jpg")),
    ]);
    adopt(&mut app, &ctx, "folder", Arc::clone(&result));
    let index = app.items.iter().position(|item| item == &row).unwrap();
    let nav = app.grid_physical_navigation(index, folder.clone(), false);
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    app.finish_page_identity_preparation_for_test();
    assert!(
        app.everything_has_physical_child(),
        "real post-install callback must accept Folder"
    );
    assert_eq!(app.current_folder.as_ref(), Some(&folder));
    assert!(
        matches!(&app.global_search.accepted, SearchAccepted::Everything {
        position: EverythingPosition::PhysicalChild { logical_source, return_anchor, .. }, ..
    } if logical_source == &folder && return_anchor == &folder)
    );
    app.load_folder(sub.clone());
    app.finish_page_identity_preparation_for_test();
    assert!(app.handle_grid_parent_folder_action().is_none());
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.current_folder.as_ref(), Some(&folder));
    assert_eq!(app.items[app.selected.unwrap()], GridItem::Folder(sub));
    assert!(app.everything_has_physical_child());
    assert!(app.global_search.pending.is_none());
    assert!(app.handle_gamepad_grid_back().is_none());
    assert_return_only(&app);
    poll_until(&mut app, &ctx, "gamepad return did not adopt", |app| {
        !app.everything_has_physical_child()
    });
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    assert_eq!(app.items[app.selected.unwrap()], row);
    assert!(app.scroll_to_selected);
    let next = dataset(vec![GridItem::Folder(app.tmp.path().join("replacement"))]);
    adopt(&mut app, &ctx, "replacement", Arc::clone(&next));
    assert!(Arc::ptr_eq(accepted_dataset(&app), &next));
    assert!(!Arc::ptr_eq(accepted_dataset(&app), &result));
    app.close_global_search();
    app.finish_history_restore_preparation_for_test();
    assert!(!app.global_search.active);
    assert_eq!(app.current_folder.as_ref(), Some(&origin));
    assert_eq!(
        app.items,
        vec![GridItem::Image(origin.join("original.jpg"))]
    );
}

#[test]
fn results_return_skips_deep_zip_and_physical_parents_of_folder_child() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    app.current_folder = Some(app.tmp.path().to_path_buf());
    let folder = app.tmp.path().join("hit");
    let zip = folder.join("sub/book.zip");
    let result = dataset(vec![GridItem::Folder(folder.clone())]);
    adopt(&mut app, &ctx, "folder", Arc::clone(&result));
    assert!(app.accept_everything_physical_child(&folder));
    let tree = Arc::new(crate::zip_tree::ZipTree::build(
        zip.clone(),
        ["root.jpg", "chapter/deep/page.jpg"]
            .map(|name| crate::zip_loader::ZipImageEntry {
                entry_name: name.into(),
                uncompressed_size: 1,
                mtime: 1,
                container_ancestry: Vec::new(),
            })
            .to_vec(),
    ));
    let mut nav = crate::zip_tree::ZipNavState::new(tree);
    nav.enter("chapter/deep/");
    app.current_folder = Some(zip);
    app.zip_nav = Some(nav);
    assert!(app.return_from_everything_child(EverythingReturn::Results));
    assert_return_only(&app);
    assert!(
        !app.zip_nav.as_ref().unwrap().at_root(),
        "Results must not step zip_nav"
    );
    poll_until(&mut app, &ctx, "direct return did not adopt", |app| {
        !app.everything_has_physical_child()
    });
    assert!(app.zip_nav.is_none());
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    assert_eq!(app.items[app.selected.unwrap()], GridItem::Folder(folder));
}

#[cfg(windows)]
#[test]
fn image_folder_book_accepts_main_install_but_independent_open_keeps_results_root() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let folder = app.tmp.path().join("image-folder");
    std::fs::create_dir(&folder).unwrap();
    image::RgbImage::new(2, 2)
        .save(folder.join("001.png"))
        .unwrap();
    app.current_folder = Some(app.tmp.path().to_path_buf());
    let result = dataset(vec![GridItem::Folder(folder.clone())]);
    adopt(&mut app, &ctx, "images", Arc::clone(&result));
    let rows = app.items.clone();
    let generation = app.items_generation;
    assert!(app.start_active_detached_book_context_from_scanned_folder(
        folder.clone(),
        crate::app::folder_scan::scan_directory(&folder),
        &ctx,
        None
    ));
    app.with_active_viewer_context(|viewer| {
        viewer.finish_page_identity_preparation_for_test();
        assert_eq!(viewer.current_folder.as_ref(), Some(&folder));
        assert!(
            viewer
                .items
                .iter()
                .any(|item| matches!(item, GridItem::Image(_)))
        );
    })
    .expect("independent viewer installed");
    assert_eq!(app.items, rows);
    assert_eq!(app.items_generation, generation);
    assert!(!app.everything_has_physical_child());
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    // Main image-book install uses the same accepted callback as an ordinary folder.
    app.settings.detached_viewer_open_images_in_window = false;
    app.pending_auto_fs_open = true;
    app.load_folder_with_scan(
        folder.clone(),
        Some(crate::app::folder_scan::scan_directory(&folder)),
    );
    app.finish_page_identity_preparation_for_test();
    assert!(app.everything_has_physical_child());
    assert_eq!(app.current_folder.as_ref(), Some(&folder));
}

#[test]
fn cold_folder_video_pin_seed_cascades_and_retargets_thumbnail_owner_on_return() {
    use crate::folder_thumb_pins::{FileKind, FolderPinSource, FolderThumbPinDb};
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let folder = app.tmp.path().join("hit");
    let child = folder.join("nested");
    std::fs::create_dir_all(&child).unwrap();
    let video = child.join("movie.mp4");
    std::fs::write(&video, b"video").unwrap();
    let pin_db =
        Arc::new(FolderThumbPinDb::open_at(&app.tmp.path().join("folder-pins.db")).unwrap());
    pin_db
        .set(
            &folder,
            &FolderPinSource::File {
                rel: "nested".into(),
                kind: FileKind::Folder,
            },
        )
        .unwrap();
    pin_db
        .set(
            &child,
            &FolderPinSource::File {
                rel: "movie.mp4".into(),
                kind: FileKind::Video,
            },
        )
        .unwrap();
    let video_db =
        crate::video_pins::VideoPinDb::open_at(&app.tmp.path().join("video-pins.db")).unwrap();
    let encode = |color: [u8; 3]| {
        let mut bytes = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut bytes)
            .encode(
                image::RgbImage::from_pixel(4, 3, image::Rgb(color)).as_raw(),
                4,
                3,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        bytes
    };
    let webp = encode([18, 90, 140]);
    video_db.set_pin(&video, 2.0, &webp).unwrap();
    app.folder_thumb_pin_db = Some(Arc::clone(&pin_db));
    app.video_pin_db = Some(video_db);
    app.settings.folder_thumb_depth = 3;
    // Non-default representative policy must use the same key as the loader.
    app.settings
        .file_type_visibility
        .excluded_extensions
        .insert("png".into());
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&app.tmp.path().join("cache"), &folder).unwrap());
    let first_cache = Arc::new(std::sync::RwLock::new(HashMap::new()));
    app.current_color_catalog = Some(Arc::clone(&catalog));
    app.current_color_cache_map = Some(Arc::clone(&first_cache));
    app.current_folder = Some(app.tmp.path().to_path_buf());
    let result = dataset(vec![
        GridItem::Folder(folder.clone()),
        GridItem::Folder(child.clone()),
    ]);
    adopt(&mut app, &ctx, "folders", Arc::clone(&result));
    let keys = first_cache
        .read()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        keys.len(),
        2,
        "direct and cascaded video pin both seed a cold cache"
    );
    for key in &keys {
        assert!(key.contains("|types:"));
        let entry = catalog.load_one(key).unwrap().unwrap();
        assert_eq!(entry.jpeg_data, webp);
        assert_eq!(
            entry.folder_provenance,
            Some(crate::catalog::FolderThumbProvenance::Seeded)
        );
        assert_eq!(first_cache.read().unwrap()[key].jpeg_data, webp);
    }
    assert!(
        keys.iter()
            .any(|key| key.contains(folder.to_string_lossy().as_ref()))
    );
    assert_eq!(
        app.video_pin_db
            .as_ref()
            .unwrap()
            .bulk_lookup_calls_for_test(),
        0,
        "UI must not read pin BLOBs from its connection"
    );
    assert!(app.accept_everything_physical_child(&folder));
    // A physical child replaces the existing thumbnail owner. Retained search prepare
    // must seed that new owner, rather than the one captured at the first query.
    let return_cache = Arc::new(std::sync::RwLock::new(HashMap::new()));
    app.current_color_cache_map = Some(Arc::clone(&return_cache));
    assert!(app.return_from_everything_child(EverythingReturn::Results));
    assert_return_only(&app);
    poll_until(&mut app, &ctx, "folder pin return did not adopt", |app| {
        !app.everything_has_physical_child()
    });
    assert_eq!(return_cache.read().unwrap().len(), 2);
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    // The source pin changes without changing video stat: replace the actual bytes.
    let replacement = encode([190, 10, 40]);
    app.video_pin_db
        .as_ref()
        .unwrap()
        .set_pin(&video, 3.0, &replacement)
        .unwrap();
    let generation = app.items_generation;
    app.rebuild_items_from_global_search();
    poll_until(&mut app, &ctx, "changed folder pin did not adopt", |app| {
        app.items_generation != generation
    });
    for key in keys {
        assert_eq!(return_cache.read().unwrap()[&key].jpeg_data, replacement);
    }
}

#[test]
fn thumbnail_owner_replaced_while_ready_is_pending_is_recaptured_on_retry() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let folder = app.tmp.path().join("folder");
    let catalog =
        Arc::new(crate::catalog::CatalogDb::open(&app.tmp.path().join("cache"), &folder).unwrap());
    app.current_color_catalog = Some(catalog);
    app.current_color_cache_map = Some(Arc::new(std::sync::RwLock::new(HashMap::new())));
    let result = dataset(vec![GridItem::Folder(folder)]);
    adopt(&mut app, &ctx, "folder", Arc::clone(&result));
    app.hold_next_search_ready_for_test();
    app.rebuild_items_from_global_search();
    poll_until(
        &mut app,
        &ctx,
        "Ready not held before owner replacement",
        App::search_ready_is_held_for_test,
    );
    let generation = app.items_generation;
    let cancel = app.search_prepare_cancel_for_test();
    let replacement = Arc::new(std::sync::RwLock::new(HashMap::new()));
    app.current_color_cache_map = Some(Arc::clone(&replacement));
    app.release_search_ready_for_test();
    poll_until(
        &mut app,
        &ctx,
        "retry kept using the retired thumbnail owner",
        |app| app.items_generation != generation,
    );
    assert!(app.search_prepare_metrics_for_test().stale_rejections > 0);
    assert!(Arc::ptr_eq(
        app.current_color_cache_map.as_ref().unwrap(),
        &replacement
    ));
    assert!(
        Arc::ptr_eq(&app.search_prepare_cancel_for_test(), &cancel),
        "reuse the existing prepare owner"
    );
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
}

#[cfg(windows)]
#[test]
fn ten_thousand_completed_rows_keep_ui_adoption_bounded_and_sibling_unchanged() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    app.current_folder = Some(app.tmp.path().to_path_buf());
    app.settings.sort_order = SortOrder::FileName;
    let sibling_pdf = app.tmp.path().join("sibling.pdf");
    let sibling = app.build_window_context_for_test(1376, |mounted| {
        mounted.items = vec![GridItem::PdfPage {
            pdf_path: sibling_pdf,
            page_num: 0,
            content_type: None,
        }];
        mounted.image_metas = vec![None];
        mounted.thumbnails = vec![ThumbnailState::Pending];
    });
    let (sibling_ticket, sibling_generation, sibling_cancel, sibling_rows) = app
        .with_window_context_for_test(sibling, |mounted| {
            (
                mounted.pdf_render_epoch.snapshot(),
                mounted.items_generation,
                Arc::clone(&mounted.cancel_token),
                mounted.items.clone(),
            )
        });
    let check_sibling = |app: &mut App| {
        app.with_window_context_for_test(sibling, |mounted| {
            assert_eq!(mounted.items, sibling_rows);
            assert_eq!(mounted.items_generation, sibling_generation);
            assert!(sibling_ticket.is_current());
            assert!(Arc::ptr_eq(&mounted.cancel_token, &sibling_cancel));
            assert!(!sibling_cancel.load(Ordering::Relaxed));
            assert!(!mounted.everything_search_progress_visible());
        });
    };
    let make_rows = |root: &Path| {
        dataset(
            (0..10_000)
                .map(|index| {
                    if index == 0 {
                        GridItem::ZipFile(root.join("00000.zip"))
                    } else if index % 2 == 1 {
                        GridItem::Video(root.join(format!("{index:05}.mp4")))
                    } else {
                        GridItem::Image(root.join(format!("{index:05}.jpg")))
                    }
                })
                .collect(),
        )
    };
    let first = make_rows(&app.tmp.path().join("first"));
    let second = make_rows(&app.tmp.path().join("second"));
    // File-backed pin store with real WebP BLOBs: exercise all bulk-read chunks
    // without depending on native Shell/video decoding or the user's profile.
    let pin_path = app.tmp.path().join("prepared-video-pins.db");
    app.video_pin_db = Some(crate::video_pins::VideoPinDb::open_at(&pin_path).unwrap());
    let mut webp = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
        .encode(&[19, 73, 127, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    let mut connection = rusqlite::Connection::open(&pin_path).unwrap();
    let transaction = connection.transaction().unwrap();
    {
        let mut insert = transaction.prepare(
            "INSERT INTO video_pins(path,pin_pts_secs,thumb_webp,thumb_pts_secs) VALUES(?1,1,?2,1)",
        ).unwrap();
        for rows in [&first, &second] {
            for row in &rows.rows {
                if let GridItem::Video(path) = &row.item {
                    insert
                        .execute(rusqlite::params![
                            crate::path_key::normalize_keep_drive(path),
                            &webp
                        ])
                        .unwrap();
                }
            }
        }
    }
    transaction.commit().unwrap();
    drop(connection);
    let first_weak = Arc::downgrade(&first);
    let mut maximum_drain = Duration::ZERO;
    let mut adoption_times = Vec::new();
    for (replacement, result) in [&first, &second].into_iter().enumerate() {
        if replacement == 1 {
            // A disappeared high ordinal exposes accidental same-list neighbor adoption.
            app.selected = Some(9_000);
        }
        start_fake_run(&mut app, &ctx, "fake completed dataset");
        app.hold_next_search_ready_for_test();
        let prior_generation = app.items_generation;
        queue(&mut app, Arc::clone(result));
        let deadline = Instant::now() + Duration::from_secs(30);
        while !app.search_ready_is_held_for_test() {
            let poll_started = Instant::now();
            app.poll_search_page_edit_prepare(&ctx);
            maximum_drain = maximum_drain.max(poll_started.elapsed());
            assert!(Instant::now() < deadline, "10,000-row Ready did not arrive");
            std::thread::yield_now();
        }
        assert_eq!(app.items_generation, prior_generation);
        let prepare = app.global_search.page_edit_prepare.as_ref().unwrap();
        let Some(SearchPrepareOutput::Ready(_, prepared)) = &prepare.held_ready_for_test else {
            panic!("expected held complete payload");
        };
        let SearchMetadataInstall::Prepared(metadata) = &prepared.metadata else {
            panic!("Everything metadata must be prepared");
        };
        assert_eq!(metadata.video_pin_blobs.len(), 5_000);
        for row in &result.rows {
            if let GridItem::Video(path) = &row.item {
                assert_eq!(metadata.video_pin_blobs.get(path), Some(&webp));
            }
        }
        assert_eq!(
            app.video_pin_db
                .as_ref()
                .unwrap()
                .bulk_lookup_calls_for_test(),
            0
        );
        check_sibling(&mut app);
        app.release_search_ready_for_test();
        let adoption_started = Instant::now();
        app.poll_search_page_edit_prepare(&ctx);
        adoption_times.push(adoption_started.elapsed());
        assert!(Arc::ptr_eq(accepted_dataset(&app), result));
        assert_eq!(app.items.len(), 10_000);
        assert_eq!(app.image_metas.len(), 10_000);
        assert_eq!(
            app.video_pin_db
                .as_ref()
                .unwrap()
                .bulk_lookup_calls_for_test(),
            0,
            "UI adoption must install worker pin BLOBs without a connection lookup"
        );
        assert_eq!(app.selected, Some(0));
        for (index, (item, stamp)) in app.items.iter().zip(&app.image_metas).enumerate() {
            assert_eq!(item, &result.rows[index].item);
            assert_eq!(*stamp, result.rows[index].image_meta);
        }
        check_sibling(&mut app);
    }
    // The outgoing accepted payload goes through the shared worker retirement path. Observe
    // eventual release without requiring that its large allocation be destroyed in UI poll.
    drop(first);
    let retire_deadline = Instant::now() + Duration::from_secs(5);
    while first_weak.upgrade().is_some() {
        assert!(
            Instant::now() < retire_deadline,
            "outgoing result payload retained"
        );
        std::thread::yield_now();
    }
    let logical_book = second.rows[0]
        .item
        .file_operation_path()
        .unwrap()
        .to_path_buf();
    assert!(app.accept_everything_physical_child(&logical_book));
    let child = GridItem::ZipImage {
        zip_path: logical_book.clone(),
        entry_name: "page.jpg".into(),
    };
    app.items = vec![child.clone()];
    app.image_metas = vec![Some((99, 99))];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.items_generation = app.items_generation.wrapping_add(1);
    app.hold_next_search_ready_for_test();
    assert!(app.return_from_everything_child(EverythingReturn::Parent));
    poll_until(
        &mut app,
        &ctx,
        "large return Ready not held",
        App::search_ready_is_held_for_test,
    );
    check_sibling(&mut app);
    app.cancel_everything_search();
    assert_eq!(app.items, vec![child]);
    assert!(app.everything_has_physical_child());
    assert!(Arc::ptr_eq(accepted_dataset(&app), &second));
    assert_eq!(
        app.video_pin_db
            .as_ref()
            .unwrap()
            .bulk_lookup_calls_for_test(),
        0,
        "retained return must also prepare video pins on the worker"
    );
    check_sibling(&mut app);
    assert!(app.return_from_everything_child(EverythingReturn::Parent));
    poll_until(
        &mut app,
        &ctx,
        "large retained return did not adopt",
        |app| !app.everything_has_physical_child(),
    );
    assert_eq!(app.items.len(), 10_000);
    assert_eq!(
        app.items[app.selected.unwrap()],
        GridItem::ZipFile(logical_book)
    );
    assert!(Arc::ptr_eq(accepted_dataset(&app), &second));
    check_sibling(&mut app);
    // Only run/context/count and timing diagnostics: no query or physical paths.
    assert_eq!(
        app.video_pin_db
            .as_ref()
            .unwrap()
            .bulk_lookup_calls_for_test(),
        0,
        "completed retained return must install worker-prepared video pins"
    );
    eprintln!(
        "Everything prepare timing source=Everything run={} context={:?} count=10000 max_drain_ms={:.3} first_adopt_ms={:.3} replacement_adopt_ms={:.3} owner_adopt_ms={:.3}",
        app.global_search.run_sequence,
        app.virtual_list_context_id(),
        maximum_drain.as_secs_f64() * 1000.0,
        adoption_times[0].as_secs_f64() * 1000.0,
        adoption_times[1].as_secs_f64() * 1000.0,
        app.search_prepare_metrics_for_test().ui_accept_ms,
    );
}

#[cfg(windows)]
#[test]
fn search_dataset_and_worker_move_with_the_owner_during_registry_park_roundtrips() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let result = dataset(vec![GridItem::Image(app.tmp.path().join("owner.jpg"))]);
    adopt(&mut app, &ctx, "owner", Arc::clone(&result));
    let prepare_cancel = app.search_prepare_cancel_for_test();
    let original_generation = app.items_generation;
    let original_context = app.virtual_list_context_id();
    let rows = app.items.clone();
    let sibling = app.build_window_context_for_test(1377, |mounted| {
        assert!(matches!(
            mounted.global_search.accepted,
            SearchAccepted::Metadata { .. }
        ));
        assert!(mounted.global_search.pending.is_none());
        assert!(mounted.global_search.page_edit_prepare.is_none());
    });
    for _ in 0..3 {
        app.with_window_context_for_test(sibling, |mounted| {
            assert!(matches!(
                mounted.global_search.accepted,
                SearchAccepted::Metadata { .. }
            ));
            assert!(mounted.global_search.pending.is_none());
            assert!(mounted.global_search.page_edit_prepare.is_none());
        });
        assert_eq!(app.virtual_list_context_id(), original_context);
        assert_eq!(app.global_search.source, SearchSource::Everything);
        assert_eq!(app.items_generation, original_generation);
        assert_eq!(app.items, rows);
        assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
        assert!(Arc::ptr_eq(
            &app.search_prepare_cancel_for_test(),
            &prepare_cancel
        ));
        assert!(!prepare_cancel.load(Ordering::Relaxed));
    }
}

#[test]
fn source_boundary_discards_index_owned_undo_before_new_rows_can_use_the_index() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let old = app.tmp.path().join("old.jpg");
    adopt(&mut app, &ctx, "old", dataset(vec![GridItem::Image(old)]));
    let target = app.page_adjustment_target_for_idx(0).unwrap();
    app.meta_undo
        .push(crate::undo_stack::UndoEntry::LocalAdjustment {
            changes: vec![crate::undo_stack::LocalAdjustmentChange {
                target,
                before: local_adjust_core::LocalAdjustmentLayers::default(),
                after: local_adjust_core::LocalAdjustmentLayers::default(),
            }],
            summary: "old index".into(),
        });
    assert_eq!(app.meta_undo.undo_len(), 1);
    app.switch_search_source(SearchSource::Metadata);
    assert_eq!(app.meta_undo.undo_len(), 0);
    app.switch_search_source(SearchSource::Everything);
    let new = GridItem::Image(app.tmp.path().join("new.jpg"));
    adopt(&mut app, &ctx, "new", dataset(vec![new.clone()]));
    app.apply_meta_undo();
    assert_eq!(app.items, vec![new]);
    assert_eq!(app.meta_undo.undo_len(), 0);
}

#[test]
fn captured_open_origin_cannot_apply_after_same_query_new_run_or_source_aba() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let logical = app.tmp.path().join("book.rar");
    let result = dataset(vec![GridItem::ConvertibleArchive {
        path: logical,
        format: crate::archive_converter::ArchiveFormat::Rar,
    }]);
    adopt(&mut app, &ctx, "same", Arc::clone(&result));
    let old_open = app.capture_search_open_origin().unwrap();
    assert!(app.search_open_origin_is_current(old_open));
    start_fake_run(&mut app, &ctx, "same");
    assert!(!app.search_open_origin_is_current(old_open));
    app.cancel_everything_search();
    assert!(!app.search_open_origin_is_current(old_open));
    let canceled_open = app.capture_search_open_origin().unwrap();
    app.switch_search_source(SearchSource::Metadata);
    app.switch_search_source(SearchSource::Everything);
    assert!(!app.search_open_origin_is_current(canceled_open));
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    assert!(!app.everything_has_physical_child());
}

#[test]
fn canceled_return_preserves_child_and_can_prepare_again_without_querying_everything() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let logical = app.tmp.path().join("book.zip");
    let result = dataset(vec![GridItem::ZipFile(logical.clone())]);
    adopt(&mut app, &ctx, "book", Arc::clone(&result));
    assert!(app.accept_everything_physical_child(&logical));
    let page = GridItem::ZipImage {
        zip_path: logical,
        entry_name: "page.jpg".into(),
    };
    app.items = vec![page.clone()];
    app.image_metas = vec![Some((99, 99))];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.items_generation = app.items_generation.wrapping_add(1);
    app.hold_next_search_ready_for_test();
    assert!(app.return_from_everything_child(EverythingReturn::Parent));
    poll_until(
        &mut app,
        &ctx,
        "return Ready not held",
        App::search_ready_is_held_for_test,
    );
    assert!(app.everything_search_progress_visible());
    assert!(app.everything_running_is_current());
    app.cancel_everything_search();
    assert!(app.everything_has_physical_child());
    assert_eq!(app.items, vec![page]);
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    assert!(!app.everything_search_progress_visible());
    assert!(app.return_from_everything_child(EverythingReturn::Parent));
    assert!(app.everything_running_is_current());
    assert!(
        app.global_search.page_edit_prepare.is_some(),
        "canceled common prepare must be recreated"
    );
    poll_until(&mut app, &ctx, "retried return did not adopt", |app| {
        !app.everything_has_physical_child()
    });
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    assert_eq!(
        app.items,
        vec![GridItem::ZipFile(app.tmp.path().join("book.zip"))]
    );
    assert_eq!(app.global_search.last_executed, "book");
}

#[test]
fn failed_return_preserves_the_accepted_physical_child_rows() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let logical = app.tmp.path().join("book.zip");
    let result = dataset(vec![GridItem::ZipFile(logical.clone())]);
    adopt(&mut app, &ctx, "book", Arc::clone(&result));
    assert!(app.accept_everything_physical_child(&logical));
    let page = GridItem::ZipImage {
        zip_path: logical,
        entry_name: "page.jpg".into(),
    };
    app.items = vec![page.clone()];
    app.image_metas = vec![Some((99, 99))];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.items_generation = app.items_generation.wrapping_add(1);
    let (prepare, _command_rx, output_tx) = stalled_prepare();
    app.global_search.page_edit_prepare = Some(prepare);
    assert!(app.return_from_everything_child(EverythingReturn::Parent));
    let wish = app
        .global_search
        .page_edit_prepare
        .as_ref()
        .unwrap()
        .desired
        .as_ref()
        .unwrap()
        .clone();
    output_tx
        .send(SearchPrepareOutput::Failed(
            wish,
            "fake prepare failure".into(),
        ))
        .unwrap();
    app.poll_search_page_edit_prepare(&ctx);
    assert!(app.everything_has_physical_child());
    assert_eq!(app.items, vec![page]);
    assert!(Arc::ptr_eq(accepted_dataset(&app), &result));
    assert!(
        !app.everything_search_progress_visible(),
        "failed return must release modal admission"
    );
}

#[cfg(windows)]
#[test]
fn main_search_adoption_and_cancel_preserve_detached_same_pdf_resources() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    let shared = app.tmp.path().join("shared.pdf");
    let sibling = app.build_window_context_for_test(1375, |mounted| {
        mounted.items = vec![GridItem::PdfPage {
            pdf_path: shared.clone(),
            page_num: 0,
            content_type: None,
        }];
        mounted.image_metas = vec![Some((10, 20))];
        mounted.thumbnails = vec![ThumbnailState::Pending];
        mounted.reload_queue = Some(Arc::new((
            std::sync::Mutex::new(Vec::new()),
            std::sync::Condvar::new(),
        )));
        mounted.heavy_io_queue = Some(Arc::new((
            std::sync::Mutex::new(Vec::new()),
            std::sync::Condvar::new(),
        )));
        mounted.ensure_still_seek_thumbnail_requests(&ctx, &[0]);
    });
    let (ticket, generation, cancel, rows, request, seek_queue) =
        app.with_window_context_for_test(sibling, |mounted| {
            (
                mounted.pdf_render_epoch.snapshot(),
                mounted.items_generation,
                Arc::clone(&mounted.cancel_token),
                mounted.items.clone(),
                mounted.requested.id(0).unwrap(),
                mounted.reload_queue.clone().unwrap(),
            )
        });
    let validity = seek_queue
        .0
        .lock()
        .unwrap()
        .pop()
        .unwrap()
        .pdf_render_validity;
    assert!(
        validity.same_request(&crate::pdf_loader::PdfRenderValidity::Viewer(
            ticket.clone()
        ))
    );
    let main_ticket = app.pdf_render_epoch.snapshot();
    let mut pdf_consumer = crate::pdf_loader::TestPdfQueue::new();
    let key = crate::grid_item::pdf_page_perf_key(&shared, 0);
    let main_reply = pdf_consumer.enqueue(
        &key,
        crate::pdf_loader::PdfRenderValidity::Viewer(main_ticket),
        crate::pdf_loader::JobPriority::Normal,
    );
    let sibling_reply =
        pdf_consumer.enqueue(&key, validity, crate::pdf_loader::JobPriority::HighNormal);
    let result = dataset(vec![GridItem::PdfFile(shared)]);
    adopt(&mut app, &ctx, "same PDF", result);
    pdf_consumer.prune_stale();
    assert!(pdf_consumer.complete_next());
    assert_eq!(
        main_reply.recv().unwrap_err().kind(),
        std::io::ErrorKind::Interrupted
    );
    sibling_reply.recv().unwrap();
    start_fake_run(&mut app, &ctx, "cancel PDF");
    assert!(app.everything_search_progress_visible());
    app.with_window_context_for_test(sibling, |mounted| {
        assert!(!mounted.everything_search_progress_visible());
        assert!(!mounted.common_modal_dialog_open());
        assert_eq!(mounted.items, rows);
        assert_eq!(mounted.items_generation, generation);
        assert_eq!(mounted.requested.id(0), Some(request));
        assert!(Arc::ptr_eq(
            mounted.reload_queue.as_ref().unwrap(),
            &seek_queue
        ));
        assert!(ticket.is_current());
        assert!(Arc::ptr_eq(&mounted.cancel_token, &cancel));
        assert!(!cancel.load(Ordering::Relaxed));
    });
    app.cancel_everything_search();
    app.with_window_context_for_test(sibling, |mounted| {
        assert_eq!(mounted.items, rows);
        assert_eq!(mounted.items_generation, generation);
        assert_eq!(mounted.requested.id(0), Some(request));
        assert!(Arc::ptr_eq(
            mounted.reload_queue.as_ref().unwrap(),
            &seek_queue
        ));
        assert!(ticket.is_current());
        assert!(Arc::ptr_eq(&mounted.cancel_token, &cancel));
        assert!(!cancel.load(Ordering::Relaxed));
    });
}
