//! 読書位置の記録、一覧への反映、path-key workerとの順序を本番入口で検証する。
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::book_resume_meter::BookResumeMeters;
use super::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::book_resume_db::{BookResumeWriter, ReadingMeterValue};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::settings::{ReadingDirection, SpreadMode};

const WORKER_TIMEOUT: Duration = Duration::from_secs(10);

fn meter(ordinal: usize, total: usize, rtl: bool) -> Option<ReadingMeterValue> {
    ReadingMeterValue::new(ordinal, total, rtl)
}

/// ReadのACKは、それより前のRecord/Clearがcommit済みであることを証明する。
/// pendingの減算はACK送信直後なので、busy predicateも解除されるまで待つ。
fn writer_ack(writer: &BookResumeWriter) {
    writer
        .read_all()
        .recv_timeout(WORKER_TIMEOUT)
        .unwrap()
        .unwrap();
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while writer.is_busy() {
        assert!(
            Instant::now() < deadline,
            "book-resume writer did not finish"
        );
        std::thread::yield_now();
    }
}

fn settle(app: &mut AppTestEnv) {
    writer_ack(app.book_resume_writer.as_ref().expect("book-resume writer"));
    app.poll_book_resume_meters(&egui::Context::default());
}

fn set_pages(app: &mut AppTestEnv, items: Vec<GridItem>) {
    app.items = items;
    app.thumbnails = (0..app.items.len())
        .map(|_| ThumbnailState::Pending)
        .collect();
    app.items_generation = app.items_generation.wrapping_add(1);
    app.rebuild_visible_indices();
}

fn stored_row(
    data_dir: &Path,
    path: &Path,
) -> Option<(i64, Option<i64>, Option<i64>, Option<i64>)> {
    use rusqlite::OptionalExtension;
    rusqlite::Connection::open(data_dir.join("book_resume.db"))
        .unwrap()
        .query_row(
            "SELECT page,page_ordinal,page_total,reading_rtl FROM book_resume WHERE path=?1",
            [crate::path_key::normalize(path)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .unwrap()
}

#[test]
fn book_resume_meter_records_hud_ordinal_in_mixed_items() {
    let mut app = setup_app();
    settle(&mut app);
    let book = app.tmp.path().join("mixed-book");
    app.current_folder = Some(book.clone());
    app.reading_direction = ReadingDirection::Ltr;
    app.spread_mode = SpreadMode::Single;
    set_pages(
        &mut app,
        vec![
            GridItem::Image(book.join("one.jpg")),
            GridItem::Video(book.join("bonus.mp4")),
            GridItem::Folder(book.join("child")),
            GridItem::ZipImage {
                zip_path: book.join("pages.zip"),
                entry_name: "two.jpg".into(),
            },
            GridItem::PdfPage {
                pdf_path: book.join("pages.pdf"),
                page_num: 7,
                content_type: None,
            },
        ],
    );

    for (idx, ordinal) in [(0, 1), (3, 2), (4, 3)] {
        assert_eq!(
            app.fullscreen_page_number_label(idx),
            Some(format!("{ordinal} / 3"))
        );
        app.record_book_resume(idx);
        assert_eq!(
            app.last_book_resume,
            Some((book.clone(), idx, meter(ordinal, 3, false)))
        );
        assert_eq!(app.book_resume_meters.get(&book), meter(ordinal, 3, false));
    }
    app.record_book_resume(1);
    app.record_book_resume(2);
    assert_eq!(
        app.last_book_resume,
        Some((book.clone(), 4, meter(3, 3, false)))
    );
    settle(&mut app);
    assert_eq!(
        stored_row(app.tmp.path(), &book),
        Some((4, Some(3), Some(3), Some(0)))
    );
}

#[test]
fn book_resume_meter_keeps_rtl_single_direction_and_spread_anchor() {
    let mut app = setup_app();
    settle(&mut app);
    let book = app.tmp.path().join("rtl-book");
    app.current_folder = Some(book.clone());
    app.reading_direction = ReadingDirection::Rtl;
    set_pages(
        &mut app,
        (1..=4)
            .map(|n| GridItem::Image(book.join(format!("{n}.jpg"))))
            .collect(),
    );

    app.spread_mode = SpreadMode::Single;
    assert_eq!(
        app.fullscreen_page_number_label(1).as_deref(),
        Some("2 / 4")
    );
    app.record_book_resume(1);
    assert_eq!(
        app.last_book_resume,
        Some((book.clone(), 1, meter(2, 4, true)))
    );

    app.spread_mode = SpreadMode::Rtl;
    assert_eq!(
        app.fullscreen_page_number_label(1).as_deref(),
        Some("2, 1 / 4")
    );
    app.record_book_resume(1);
    assert_eq!(
        app.last_book_resume,
        Some((book.clone(), 1, meter(2, 4, true)))
    );
    assert_eq!(app.book_resume_meters.get(&book), meter(2, 4, true));
}

#[test]
fn book_resume_meter_same_raw_index_updates_denominator_direction_and_null() {
    let mut app = setup_app();
    settle(&mut app);
    let book = app.tmp.path().join("changing-book");
    app.current_folder = Some(book.clone());
    app.reading_direction = ReadingDirection::Ltr;
    set_pages(&mut app, vec![GridItem::Image(book.join("one.jpg"))]);
    app.record_book_resume(0);
    settle(&mut app);
    assert_eq!(
        stored_row(app.tmp.path(), &book),
        Some((0, Some(1), Some(1), Some(0)))
    );

    set_pages(
        &mut app,
        vec![
            GridItem::Image(book.join("one.jpg")),
            GridItem::Image(book.join("two.jpg")),
        ],
    );
    app.record_book_resume(0);
    settle(&mut app);
    assert_eq!(
        stored_row(app.tmp.path(), &book),
        Some((0, Some(1), Some(2), Some(0)))
    );

    app.reading_direction = ReadingDirection::Rtl;
    app.record_book_resume(0);
    settle(&mut app);
    assert_eq!(
        stored_row(app.tmp.path(), &book),
        Some((0, Some(1), Some(2), Some(1)))
    );

    // 共通入口のNULL記録 (Remoteもこの入口) は同idxでも旧meterを消す。
    app.persist_book_resume(book.clone(), 0, None);
    assert_eq!(app.book_resume_meters.get(&book), None);
    settle(&mut app);
    assert_eq!(
        stored_row(app.tmp.path(), &book),
        Some((0, None, None, None))
    );
    assert_eq!(app.book_resume_entry_count(), 1, "NULL行も登録件数には含む");
}

#[test]
fn book_resume_meter_pending_snapshot_replays_record_remove_and_clear() {
    let mut app = setup_app();
    settle(&mut app);
    let keep = app.tmp.path().join("keep.zip");
    let remove = app.tmp.path().join("remove.zip");
    app.persist_book_resume(keep.clone(), 0, meter(1, 3, false));
    app.persist_book_resume(remove.clone(), 0, meter(1, 4, false));
    settle(&mut app);

    // 古いSELECT結果を受信済みだが未適用のまま、ローカルの更新を先に適用する。
    app.reload_book_resume_meters();
    writer_ack(app.book_resume_writer.as_ref().unwrap());
    app.persist_book_resume(keep.clone(), 2, meter(3, 3, true));
    app.book_resume_meters
        .remove_scopes(std::slice::from_ref(&remove));
    app.poll_book_resume_meters(&egui::Context::default());
    assert_eq!(app.book_resume_meters.get(&keep), meter(3, 3, true));
    assert_eq!(app.book_resume_meters.get(&remove), None);
    assert_eq!(app.book_resume_entry_count(), 1);

    app.reload_book_resume_meters();
    writer_ack(app.book_resume_writer.as_ref().unwrap());
    app.clear_book_resume_async();
    assert_eq!(app.last_book_resume, None);
    app.persist_book_resume(keep.clone(), 1, meter(2, 3, false));
    app.poll_book_resume_meters(&egui::Context::default());
    assert_eq!(app.book_resume_meters.get(&keep), meter(2, 3, false));
    assert_eq!(app.book_resume_meters.get(&remove), None);
    assert_eq!(app.book_resume_entry_count(), 1);
    settle(&mut app);
    assert_eq!(stored_row(app.tmp.path(), &remove), None);
    assert_eq!(
        stored_row(app.tmp.path(), &keep),
        Some((1, Some(2), Some(3), Some(0)))
    );
}

#[test]
fn book_resume_meter_new_reload_replaces_old_receiver() {
    let dir = tempfile::tempdir().unwrap();
    let writer = BookResumeWriter::spawn_at(dir.path().join("book_resume.db")).unwrap();
    let book = PathBuf::from("C:/books/reload.zip");
    assert!(writer.record(&book, 0, meter(1, 3, false)));
    let mut cache = BookResumeMeters::default();
    cache.reload(&writer);
    writer_ack(&writer);
    assert!(writer.record(&book, 2, meter(3, 3, true)));
    cache.reload(&writer);
    writer_ack(&writer);
    assert!(cache.poll().unwrap().is_ok());
    assert_eq!(cache.get(&book), meter(3, 3, true));
    assert_eq!(cache.count(), 1);
}

#[test]
fn book_resume_meter_reload_preserves_latest_pending_resume_source() {
    let mut app = setup_app();
    settle(&mut app);
    let book = app.tmp.path().join("pending-book.zip");
    let resume = app.book_resume_writer.as_ref().unwrap().pause_for_test();
    let value = meter(8, 10, true);
    app.persist_book_resume(book.clone(), 7, value);
    let latest = Some((book.clone(), 7, value));
    app.reload_book_resume_meters();
    assert_eq!(
        app.last_book_resume, latest,
        "Remoteの直近raw位置は再読込で失わない"
    );
    assert_eq!(app.book_resume_meters.get(&book), value);

    resume.send(()).unwrap();
    settle(&mut app);
    assert_eq!(app.last_book_resume, latest);
    assert_eq!(app.book_resume_meters.get(&book), value);
    assert_eq!(
        stored_row(app.tmp.path(), &book),
        Some((7, Some(8), Some(10), Some(1)))
    );
}

#[test]
fn book_resume_meter_is_hidden_on_synthetic_surfaces_and_virtual_container_grids() {
    use super::top_level_grid_view::{
        CollectionGridIdentity, CollectionGridPosition, SmartFolderViewState, TopLevelGridSurface,
        TopLevelSearchView,
    };

    let mut app = setup_app();
    settle(&mut app);
    let book = app.tmp.path().join("surface-book");
    let value = meter(2, 3, false);
    app.persist_book_resume(book.clone(), 1, value);
    set_pages(&mut app, vec![GridItem::Folder(book.clone())]);
    assert_eq!(app.thumbnail_book_resume_meter(0), value);

    // 旧来のitems_are_*フラグが立たないSnapshot/Smart/Collectionもsurfaceで除外する。
    for surface in [
        TopLevelGridSurface::Snapshot,
        TopLevelGridSurface::Rating { stars: 3 },
        TopLevelGridSurface::SmartFolder(SmartFolderViewState::root(uuid::Uuid::nil(), Vec::new())),
        TopLevelGridSurface::SubfolderExpansion,
        TopLevelGridSurface::Search(TopLevelSearchView::Global),
        TopLevelGridSurface::Search(TopLevelSearchView::Favorite),
        TopLevelGridSurface::Search(TopLevelSearchView::Tag),
        TopLevelGridSurface::ReadingHistory,
        TopLevelGridSurface::Bookmarks,
        TopLevelGridSurface::DriveList,
        TopLevelGridSurface::Collection(CollectionGridIdentity {
            collection_id: crate::collection_store::CollectionId::new(),
        }),
    ] {
        app.top_level_grid_view.replace_surface(surface.clone());
        assert_eq!(app.thumbnail_book_resume_meter(0), None, "{surface:?}");
        assert_eq!(app.book_resume_meters.get(&book), value);
    }
    app.top_level_grid_view
        .replace_surface(TopLevelGridSurface::Folder);
    assert_eq!(app.thumbnail_book_resume_meter(0), value);

    // タグ検索sessionを残して物理子に入っていても、合成rootではない。
    app.current_folder = Some(book.clone());
    app.tag_view.active = true;
    app.tag_view.nav_stack.push(book.clone());
    app.items_are_tag_view = false;
    assert_eq!(app.thumbnail_book_resume_meter(0), value);
    app.tag_view.active = false;
    app.tag_view.nav_stack.clear();

    let mut smart = SmartFolderViewState::root(uuid::Uuid::nil(), vec![book.clone()]);
    assert!(smart.enter_containing_path(&book));
    app.top_level_grid_view
        .replace_surface(TopLevelGridSurface::SmartFolder(smart));
    assert_eq!(app.thumbnail_book_resume_meter(0), value);

    app.top_level_grid_view
        .replace_surface(TopLevelGridSurface::Collection(CollectionGridIdentity {
            collection_id: crate::collection_store::CollectionId::new(),
        }));
    app.top_level_grid_view
        .collection_session_mut()
        .unwrap()
        .position = CollectionGridPosition::PhysicalSource {
        entry_id: crate::collection_store::CollectionEntryId::new(),
        source_key: crate::collection_store::CollectionSourcePath::from_trusted(&book)
            .unwrap()
            .key()
            .clone(),
        path: book.clone(),
    };
    assert_eq!(app.thumbnail_book_resume_meter(0), value);
    app.top_level_grid_view
        .replace_surface(TopLevelGridSurface::Folder);

    app.current_folder = Some(app.tmp.path().join("open.pdf"));
    assert!(app.grid_is_pdf_pages());
    assert_eq!(app.thumbnail_book_resume_meter(0), None);
    app.current_folder = None;
    let zip = app.tmp.path().join("open.zip");
    set_pages(
        &mut app,
        vec![
            GridItem::ZipImage {
                zip_path: zip,
                entry_name: "one.jpg".into(),
            },
            GridItem::Folder(book),
        ],
    );
    assert!(app.grid_is_zip_entries());
    assert_eq!(app.thumbnail_book_resume_meter(1), None);
}

#[test]
fn book_resume_meter_scope_removal_respects_boundaries_and_tile_kind() {
    let mut app = setup_app();
    settle(&mut app);
    let root = app.tmp.path().join("book.zip");
    let nested = PathBuf::from(format!("{}::child", root.display()));
    let child = root.join("child");
    let sibling = app.tmp.path().join("book.zip2");
    let value = meter(2, 3, false);
    for path in [&root, &nested, &child, &sibling] {
        app.persist_book_resume(path.clone(), 1, value);
    }
    app.purge_video_resume_positions_for_removed_paths(std::slice::from_ref(&root));
    for path in [&root, &nested, &child] {
        assert_eq!(app.book_resume_meters.get(path), None);
    }
    assert_eq!(app.book_resume_meters.get(&sibling), value);

    set_pages(
        &mut app,
        vec![
            GridItem::Folder(sibling.clone()),
            GridItem::ZipFile(sibling.clone()),
            GridItem::PdfFile(sibling.clone()),
            GridItem::Image(sibling.clone()),
            GridItem::Video(sibling.clone()),
            GridItem::ZipImage {
                zip_path: sibling.clone(),
                entry_name: "one.jpg".into(),
            },
            GridItem::PdfPage {
                pdf_path: sibling.clone(),
                page_num: 0,
                content_type: None,
            },
            GridItem::Stack {
                key: "stack".into(),
                representative: sibling.clone(),
                count: 2,
            },
            GridItem::ConvertibleArchive {
                path: sibling.clone(),
                format: crate::archive_converter::ArchiveFormat::Rar,
            },
        ],
    );
    for idx in 0..app.items.len() {
        assert_eq!(
            app.thumbnail_book_resume_meter(idx),
            if idx < 3 { value } else { None }
        );
    }
    app.settings.thumb_show_book_resume_meter = false;
    assert_eq!(app.thumbnail_book_resume_meter(0), None);
    app.settings.thumb_show_book_resume_meter = true;
    assert_eq!(app.thumbnail_book_resume_meter(0), value);

    // idxが同じでもitemsを入れ替えたらpath自身のmemoで照合する。
    app.items[0] = GridItem::Folder(root.clone());
    assert_eq!(app.thumbnail_book_resume_meter(0), None);
    app.items[0] = GridItem::Folder(sibling);
    assert_eq!(app.thumbnail_book_resume_meter(0), value);
    app.items_are_global_search_view = true;
    assert_eq!(app.thumbnail_book_resume_meter(0), None);
}

#[test]
fn book_resume_meter_rename_waits_for_unprocessed_record_and_moves_latest_row() {
    let mut app = setup_app();
    settle(&mut app);
    let dir = app.tmp.path().to_path_buf();
    app.rename_migration_data_dir_override = Some(dir.clone());
    let old = dir.join("old.zip");
    let new = dir.join("new.zip");
    app.persist_book_resume(old.clone(), 0, meter(1, 5, false));
    settle(&mut app);
    let resume = app.book_resume_writer.as_ref().unwrap().pause_for_test();
    app.persist_book_resume(old.clone(), 3, meter(4, 5, true));
    assert!(app.rename_migration_writers_busy());

    app.spawn_rename_key_migration(
        old.clone(),
        new.clone(),
        crate::collection_store::CollectionSourceMigrationScope::Exact,
    );
    app.flush_rename_migration_journal().unwrap();
    app.try_start_next_rename_migration();
    assert!(app.rename_migration_in_flight.is_none());
    assert_eq!(app.rename_migration_queue.len(), 1);
    assert_eq!(
        stored_row(&dir, &old),
        Some((0, Some(1), Some(5), Some(0))),
        "queued Record remains unprocessed"
    );

    resume.send(()).unwrap();
    writer_ack(app.book_resume_writer.as_ref().unwrap());
    app.try_start_next_rename_migration();
    assert!(app.rename_migration_in_flight.is_some());
    let ctx = egui::Context::default();
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while app.rename_migration_in_flight.is_some() || !app.rename_migration_queue.is_empty() {
        assert!(Instant::now() < deadline, "rename did not finish");
        app.poll_rename_migration_pending(&ctx);
        std::thread::yield_now();
    }
    settle(&mut app);
    assert_eq!(stored_row(&dir, &old), None);
    assert_eq!(stored_row(&dir, &new), Some((3, Some(4), Some(5), Some(1))));
    assert_eq!(app.book_resume_meters.get(&old), None);
    assert_eq!(app.book_resume_meters.get(&new), meter(4, 5, true));
}

#[test]
fn book_resume_meter_purge_retry_waits_for_unprocessed_record() {
    let mut app = setup_app();
    settle(&mut app);
    let dir = app.tmp.path().to_path_buf();
    app.rename_migration_data_dir_override = Some(dir.clone());
    let deleted = dir.join("deleted.zip");
    let sibling = dir.join("deleted.zip2");
    app.persist_book_resume(deleted.clone(), 0, meter(1, 5, false));
    app.persist_book_resume(sibling.clone(), 1, meter(2, 4, false));
    settle(&mut app);
    assert!(crate::metadata_cleanup::journal_failed_delete_purge(
        &dir,
        std::slice::from_ref(&deleted),
        &[]
    ));
    let resume = app.book_resume_writer.as_ref().unwrap().pause_for_test();
    app.persist_book_resume(deleted.clone(), 3, meter(4, 5, true));
    app.delete_purge_retry_needed = true;
    app.last_input_at = None;
    let ctx = egui::Context::default();
    app.poll_delete_purge_retry(&ctx);
    assert!(app.delete_purge_retry_pending.is_none());
    assert!(app.delete_purge_retry_needed);
    assert_eq!(
        stored_row(&dir, &deleted),
        Some((0, Some(1), Some(5), Some(0)))
    );

    resume.send(()).unwrap();
    writer_ack(app.book_resume_writer.as_ref().unwrap());
    app.poll_delete_purge_retry(&ctx);
    assert!(app.delete_purge_retry_pending.is_some());
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while app.delete_purge_retry_pending.is_some() {
        assert!(Instant::now() < deadline, "purge retry did not finish");
        app.poll_delete_purge_retry(&ctx);
        std::thread::yield_now();
    }
    assert!(!app.delete_purge_retry_needed);
    settle(&mut app);
    assert_eq!(
        stored_row(&dir, &deleted),
        None,
        "latest old-path Record cannot resurrect purged row"
    );
    assert_eq!(
        stored_row(&dir, &sibling),
        Some((1, Some(2), Some(4), Some(0)))
    );
    assert_eq!(app.book_resume_meters.get(&deleted), None);
    assert_eq!(app.book_resume_meters.get(&sibling), meter(2, 4, false));
}
