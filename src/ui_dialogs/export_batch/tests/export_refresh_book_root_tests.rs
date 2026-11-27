use super::*;
use crate::app::book_append_refresh::{FolderRefreshCause, FolderRefreshRequest};
use crate::export_dialog::ExportOrigin;
use crate::grid_item::{GridItem, ThumbnailState};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

fn physical_listing(relative: &str) -> (crate::app::AppTestEnvForTest, PathBuf, PathBuf) {
    let mut app = crate::app::setup_app_for_test();
    let books = app.tmp.path().join("books");
    // Keep the root's spelling identical to an output's parent (no trailing separator).
    let folder = if relative.is_empty() {
        books.clone()
    } else {
        books.join(relative)
    };
    std::fs::create_dir_all(&folder).unwrap();
    app.settings.book_root = Some(books);
    app.settings.sort_order = crate::settings::SortOrder::FileName;
    let first = folder.join("a.png");
    let output = folder.join("z-export.png");
    for path in [&first, &output] {
        image::RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 255]))
            .save(path)
            .unwrap();
    }
    app.current_folder = Some(folder.clone());
    app.current_folder_last_mtime = Some(SystemTime::UNIX_EPOCH);
    app.items = vec![GridItem::Image(first)];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.visible_indices = vec![0];
    app.selected = Some(0);
    assert!(app.current_favorite_target().is_none());
    (app, folder, output)
}

fn send_success(tx: &mpsc::Sender<ExportEvent>, output: &Path) {
    tx.send(ExportEvent::Completed(ExportSuccess {
        label: "export".into(),
        path: output.to_path_buf(),
    }))
    .unwrap();
}

fn assert_retained_operation(app: &App, folder: &Path) {
    let request = app
        .folder_refresh_pending
        .as_ref()
        .expect("matching output must retain a refresh");
    assert_eq!(request.folder, folder);
    assert!(matches!(request.cause, FolderRefreshCause::Operation));
    assert!(app.external_rescan_pending.is_none());
    assert!(app.post_operation_selection.is_none());
}

fn consume_operation_and_assert_output(app: &mut App, folder: &Path, output: &Path) {
    let first = app.items[app.selected.unwrap()].clone();
    let ctx = egui::Context::default();
    app.consume_folder_refresh_pending(&ctx);
    assert!(app.folder_refresh_pending.is_none());
    let scan = app
        .external_rescan_pending
        .as_ref()
        .expect("retained refresh must start a scan");
    assert_eq!(scan.folder, folder);
    assert!(matches!(scan.cause, FolderRefreshCause::Operation));
    assert!(!scan.restart_requested);
    assert!(app.post_operation_selection.is_none());
    crate::app::tests::phase_c_support::wait_for_external_rescan(app);
    app.finish_history_restore_preparation_for_test();
    assert!(app.items.iter().any(|item| {
        matches!(item, GridItem::Image(path) if crate::folder_tree::path_eq(path, output))
    }));
    assert!(app.post_operation_selection.is_none());
    assert_eq!(
        app.items[app.selected.unwrap()],
        first,
        "export keeps the cursor"
    );

    let generation = app.items_generation;
    app.poll_export_pending(&ctx);
    app.consume_folder_refresh_pending(&ctx);
    assert!(app.folder_refresh_pending.is_none());
    assert!(app.external_rescan_pending.is_none());
    assert_eq!(
        app.items_generation, generation,
        "refresh only once per batch"
    );
}

#[test]
fn export_refresh_book_root_batch_completion_rescans_without_selecting_output() {
    for relative in ["", "book", "book/nested"] {
        for end in [ExportEnd::Done, ExportEnd::Cancelled] {
            let (mut app, folder, output) = physical_listing(relative);
            let ctx = egui::Context::default();
            let (mut p, tx) = pending();
            p.total = 2;
            app.export_pending = Some(p);
            // The first success can belong to another folder (same-as-source export).
            let other = app.tmp.path().join("other");
            std::fs::create_dir(&other).unwrap();
            let other_output = other.join("export.png");
            std::fs::copy(&output, &other_output).unwrap();
            send_success(&tx, &other_output);
            send_success(&tx, &output);
            app.poll_export_pending(&ctx);
            assert!(
                app.folder_refresh_pending.is_none(),
                "wait for a terminal event"
            );
            assert!(app.external_rescan_pending.is_none());
            assert!(app.post_operation_selection.is_none());
            tx.send(if end == ExportEnd::Done {
                ExportEvent::AllDone
            } else {
                ExportEvent::Cancelled
            })
            .unwrap();
            app.poll_export_pending(&ctx);
            assert!(app.export_pending.is_none());
            assert_retained_operation(&app, &folder);
            consume_operation_and_assert_output(&mut app, &folder, &output);
        }
    }
}

#[test]
fn export_refresh_book_root_single_completion_is_consumed_after_main_viewer_closes() {
    for relative in ["", "book", "book/nested"] {
        let (mut app, folder, output) = physical_listing(relative);
        let ctx = egui::Context::default();
        app.fullscreen_idx = Some(0);
        app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
        assert!(app.viewer_session_blocks_main_window());
        let generation = app.items_generation;
        let (mut p, tx) = pending();
        p.origin = ExportOrigin::Single;
        p.total = 1;
        app.export_pending = Some(p);
        send_success(&tx, &output);
        tx.send(ExportEvent::AllDone).unwrap();
        app.poll_export_pending(&ctx);
        assert!(app.export_pending.is_none());
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.items_generation, generation);
        assert_retained_operation(&app, &folder);
        app.poll_export_pending(&ctx);
        assert_retained_operation(&app, &folder);

        // The main update loop consumes deferred refreshes after returning to the grid.
        app.close_fullscreen();
        assert!(!app.viewer_session_blocks_main_window());
        assert_retained_operation(&app, &folder);
        consume_operation_and_assert_output(&mut app, &folder, &output);
    }
}

#[test]
fn export_refresh_book_root_rejects_other_folder_and_virtual_or_results_listings() {
    for case in [
        "other-folder",
        "zip",
        "pdf",
        "global-results",
        "global-active",
        "favorite-results",
        "tag-results",
        "tag-rows",
        "unconfirmed-folder",
    ] {
        for origin in [ExportOrigin::Batch, ExportOrigin::Single] {
            let (mut app, _, mut output) = physical_listing("book/nested");
            match case {
                "other-folder" => {
                    let other = app.tmp.path().join("other");
                    std::fs::create_dir(&other).unwrap();
                    let other_output = other.join("export.png");
                    std::fs::copy(&output, &other_output).unwrap();
                    output = other_output;
                }
                "zip" | "pdf" => {
                    app.archive_source_override = Some(app.tmp.path().join(format!("book.{case}")));
                }
                "global-results" => app.items_are_global_search_view = true,
                "global-active" => app.global_search.active = true,
                "favorite-results" => app.favsearch.active = true,
                "tag-results" => app.tag_view.active = true,
                "tag-rows" => app.items_are_tag_view = true,
                "unconfirmed-folder" => app.current_folder_last_mtime = None,
                _ => unreachable!(),
            }
            assert!(
                app.operation_output_listing_folder(&output).is_none(),
                "{case}"
            );
            // Check the direct registration entry as well as poll's success search.
            app.note_exported_file_for_folder_refresh(&output);
            assert!(app.folder_refresh_pending.is_none(), "{case}");
            let (mut p, tx) = pending();
            p.origin = origin;
            p.total = 1;
            app.export_pending = Some(p);
            send_success(&tx, &output);
            tx.send(ExportEvent::AllDone).unwrap();
            let ctx = egui::Context::default();
            app.poll_export_pending(&ctx);
            app.consume_folder_refresh_pending(&ctx);
            assert!(app.export_pending.is_none(), "{case}");
            assert!(app.folder_refresh_pending.is_none(), "{case}");
            assert!(app.external_rescan_pending.is_none(), "{case}");
            assert!(app.post_operation_selection.is_none(), "{case}");
        }
    }
}

#[test]
fn export_refresh_book_root_retained_operation_is_consumed_in_non_book_descendant() {
    let (mut app, folder, output) = physical_listing("book/nested");
    assert!(!app.current_folder_is_book_folder());
    app.retain_folder_refresh(FolderRefreshRequest {
        folder: folder.clone(),
        cause: FolderRefreshCause::Operation,
    });
    assert_retained_operation(&app, &folder);
    consume_operation_and_assert_output(&mut app, &folder, &output);
}
