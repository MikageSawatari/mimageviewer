use super::*;
use crate::export_dialog::{ExportEvent, ExportOrigin, ExportPending, ExportSuccess};
use crate::post_operation_selection::ExpectedOutputs;
use crate::ui_dialogs::image_combine::CombineSession;

fn listed_output(new_folder: bool) -> (crate::app::AppTestEnvForTest, PathBuf, PathBuf) {
    let mut app = crate::app::setup_app_for_test();
    let books = app.tmp.path().join("books");
    let folder = if new_folder {
        app.tmp.path().join("ordinary")
    } else {
        books.join("book")
    };
    std::fs::create_dir_all(&folder).unwrap();
    app.settings.book_root = Some(books);
    app.settings.sort_order = crate::settings::SortOrder::FileName;
    let first = folder.join("a.png");
    let output = folder.join(if new_folder { "new-folder" } else { "z.png" });
    image::RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 255]))
        .save(&first)
        .unwrap();
    if new_folder {
        std::fs::create_dir(&output).unwrap();
    } else {
        image::RgbaImage::from_pixel(1, 1, image::Rgba([40, 50, 60, 255]))
            .save(&output)
            .unwrap();
    }
    // A watcher has already published the output before the operation terminal.
    // Seed the same row baseline that production rescan equality compares.
    let scan = crate::app::folder_scan::scan_directory(&folder);
    let listing = crate::app::folder_scan::materialize_local_folder_listing(
        &folder,
        scan.clone_for_projection(),
        &app.settings,
        crate::folder_order::FolderOrderListingInput::NotApplicable,
    );
    app.current_folder = Some(folder.clone());
    app.current_folder_last_mtime = Some(std::time::SystemTime::UNIX_EPOCH);
    app.current_folder_signature = Some(crate::app::folder_scan::signature_from_scan(&scan));
    app.items = listing.items;
    app.image_metas = listing.metas;
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.visible_indices = (0..app.items.len()).collect();
    app.seed_folder_observation_for_test(&folder, &scan);
    let cursor = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(first.as_path()))
        .unwrap();
    app.move_grid_cursor(Some(cursor), GridCursorIntent::Automatic);
    assert_ne!(app.selected_real_file_path().as_ref(), Some(&output));
    (app, folder, output)
}

fn complete_combine(app: &mut App, output: &Path, ctx: &egui::Context) {
    let (tx, rx) = mpsc::channel();
    app.export_pending = Some(ExportPending {
        origin: ExportOrigin::Combine,
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
        total: 2,
        done: 0,
        last_message: String::new(),
        successes: Vec::new(),
        errors: Vec::new(),
        skipped: Vec::new(),
        thumbnail_notices: Vec::new(),
        end: None,
        cancel_requested: false,
    });
    app.image_combine.session = Some(CombineSession::Writing { request: 1 });
    tx.send(ExportEvent::Completed(ExportSuccess {
        label: "combine".into(),
        path: output.to_path_buf(),
    }))
    .unwrap();
    tx.send(ExportEvent::AllDone).unwrap();
    app.poll_export_pending(ctx);
    assert!(app.export_pending.is_none());
    assert!(app.external_rescan_pending.is_some());
}

fn assert_applied_without_install(app: &App, output: &Path, generation: u64) {
    assert_eq!(
        app.items_generation, generation,
        "Equal must not reinstall the list"
    );
    assert_eq!(app.selected_real_file_path().as_deref(), Some(output));
    assert!(app.checked.is_empty());
    let request = app.post_operation_selection.as_ref().unwrap();
    assert!(request.has_applied());
    assert!(request.still_owns_selection(Some(output), &[]));
}

#[test]
fn image_combine_equal_rescan_applies_known_selection_for_already_listed_output() {
    let (mut app, _, output) = listed_output(false);
    assert!(app.current_favorite_target().is_none());
    let generation = app.items_generation;
    complete_combine(&mut app, &output, &egui::Context::default());
    phase_c_support::wait_for_external_rescan(&mut app);
    assert_applied_without_install(&app, &output, generation);
}

#[test]
fn image_combine_equal_rescan_applies_existing_new_folder_completion_selection() {
    let (mut app, folder, output) = listed_output(true);
    let generation = app.items_generation;
    let ctx = egui::Context::default();
    let (tx, rx) = mpsc::channel();
    app.new_folder_pending = Some(rx);
    assert!(
        tx.send(Ok(crate::ui_dialogs::new_folder::NewFolderCreated {
            parent: folder.clone(),
            path: output.clone()
        }))
        .is_ok()
    );
    app.poll_new_folder_pending(&ctx);
    assert!(app.pending_reload);
    assert!(!app.post_operation_selection.as_ref().unwrap().has_applied());
    // An operation rescan can finish before the queued full reload is consumed.
    app.retain_folder_refresh(book_append_refresh::FolderRefreshRequest {
        folder,
        cause: book_append_refresh::FolderRefreshCause::Operation,
    });
    app.consume_folder_refresh_pending(&ctx);
    phase_c_support::wait_for_external_rescan(&mut app);
    assert_applied_without_install(&app, &output, generation);
}

#[test]
fn image_combine_equal_rescan_defers_selection_while_main_viewer_owns_it() {
    let (mut app, _, output) = listed_output(false);
    let ctx = egui::Context::default();
    let generation = app.items_generation;
    let cursor = app.selected;
    app.fullscreen_idx = cursor;
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    assert!(app.viewer_session_blocks_main_window());
    complete_combine(&mut app, &output, &ctx);
    phase_c_support::wait_for_external_rescan(&mut app);
    assert_eq!(app.selected, cursor);
    assert!(!app.post_operation_selection.as_ref().unwrap().has_applied());
    assert!(app.folder_refresh_pending.is_some());
    app.close_fullscreen();
    app.consume_folder_refresh_pending(&ctx);
    phase_c_support::wait_for_external_rescan(&mut app);
    assert_applied_without_install(&app, &output, generation);
}

#[test]
fn image_combine_equal_rescan_does_not_take_back_user_changed_selection() {
    let (mut app, folder, output) = listed_output(false);
    let ctx = egui::Context::default();
    app.request_post_operation_selection(folder.clone(), ExpectedOutputs::Known(vec![output]));
    assert!(app.apply_post_operation_selection());
    let other = app
        .items
        .iter()
        .position(|item| item.name() == "a.png")
        .unwrap();
    app.move_grid_cursor(Some(other), GridCursorIntent::Automatic);
    app.start_external_rescan_with_cause(
        &ctx,
        folder,
        std::time::SystemTime::UNIX_EPOCH,
        book_append_refresh::FolderRefreshCause::Operation,
    );
    phase_c_support::wait_for_external_rescan(&mut app);
    assert_eq!(app.selected, Some(other));
    assert!(app.post_operation_selection.is_none());
}
