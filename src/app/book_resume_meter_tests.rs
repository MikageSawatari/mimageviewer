//! 読書位置の記録、一覧への反映、path-key workerとの順序を本番入口で検証する。
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::book_resume_meter::BookResumeMeters;
use super::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::book_resume_db::{BookResumeWriter, ReadingMeterValue};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::settings::{ReadingDirection, SpreadMode};

const WORKER_TIMEOUT: Duration = Duration::from_secs(10);

fn meter(ordinal: usize, total: usize) -> Option<ReadingMeterValue> {
    ReadingMeterValue::new(ordinal, total)
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

fn stored_row(data_dir: &Path, path: &Path) -> Option<(i64, Option<i64>, Option<i64>)> {
    use rusqlite::OptionalExtension;
    rusqlite::Connection::open(data_dir.join("book_resume.db"))
        .unwrap()
        .query_row(
            "SELECT page,page_ordinal,page_total FROM book_resume WHERE path=?1",
            [crate::path_key::normalize(path)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
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
            Some((book.clone(), idx, meter(ordinal, 3)))
        );
        assert_eq!(app.book_resume_meters.get(&book), meter(ordinal, 3));
    }
    app.record_book_resume(1);
    app.record_book_resume(2);
    assert_eq!(app.last_book_resume, Some((book.clone(), 4, meter(3, 3))));
    settle(&mut app);
    assert_eq!(
        stored_row(app.tmp.path(), &book),
        Some((4, Some(3), Some(3)))
    );
}

#[test]
fn book_resume_meter_uses_hud_rtl_reading_order_and_spread_anchor() {
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
    assert_eq!(app.last_book_resume, Some((book.clone(), 1, meter(2, 4))));

    app.spread_mode = SpreadMode::Rtl;
    assert_eq!(
        app.fullscreen_page_number_label(1).as_deref(),
        Some("2, 1 / 4")
    );
    app.record_book_resume(1);
    assert_eq!(app.last_book_resume, Some((book.clone(), 1, meter(2, 4))));
    assert_eq!(app.book_resume_meters.get(&book), meter(2, 4));
}

#[test]
fn book_resume_meter_same_raw_index_updates_denominator_and_null() {
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
        Some((0, Some(1), Some(1)))
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
        Some((0, Some(1), Some(2)))
    );

    app.reading_direction = ReadingDirection::Rtl;
    for seek_direction in [
        crate::settings::FullscreenSeekDirection::FollowReading,
        crate::settings::FullscreenSeekDirection::LeftToRight,
    ] {
        app.settings.fullscreen_seek_direction = seek_direction;
        app.record_book_resume(0);
        settle(&mut app);
        assert_eq!(
            stored_row(app.tmp.path(), &book),
            Some((0, Some(1), Some(2)))
        );
        assert_eq!(app.book_resume_meters.get(&book), meter(1, 2));
    }

    // 共通入口のNULL記録 (Remoteもこの入口) は同idxでも旧meterを消す。
    app.persist_book_resume(book.clone(), 0, None);
    assert_eq!(app.book_resume_meters.get(&book), None);
    settle(&mut app);
    assert_eq!(stored_row(app.tmp.path(), &book), Some((0, None, None)));
    assert_eq!(app.book_resume_entry_count(), 1, "NULL行も登録件数には含む");
}

#[test]
fn book_resume_meter_pending_snapshot_replays_record_remove_and_clear() {
    let mut app = setup_app();
    settle(&mut app);
    let keep = app.tmp.path().join("keep.zip");
    let remove = app.tmp.path().join("remove.zip");
    app.persist_book_resume(keep.clone(), 0, meter(1, 3));
    app.persist_book_resume(remove.clone(), 0, meter(1, 4));
    settle(&mut app);

    // 古いSELECT結果を受信済みだが未適用のまま、ローカルの更新を先に適用する。
    app.reload_book_resume_meters();
    writer_ack(app.book_resume_writer.as_ref().unwrap());
    app.persist_book_resume(keep.clone(), 2, meter(3, 3));
    app.book_resume_meters
        .remove_scopes(std::slice::from_ref(&remove));
    app.poll_book_resume_meters(&egui::Context::default());
    assert_eq!(app.book_resume_meters.get(&keep), meter(3, 3));
    assert_eq!(app.book_resume_meters.get(&remove), None);
    assert_eq!(app.book_resume_entry_count(), 1);

    app.reload_book_resume_meters();
    writer_ack(app.book_resume_writer.as_ref().unwrap());
    app.clear_book_resume_async();
    assert_eq!(app.last_book_resume, None);
    app.persist_book_resume(keep.clone(), 1, meter(2, 3));
    app.poll_book_resume_meters(&egui::Context::default());
    assert_eq!(app.book_resume_meters.get(&keep), meter(2, 3));
    assert_eq!(app.book_resume_meters.get(&remove), None);
    assert_eq!(app.book_resume_entry_count(), 1);
    settle(&mut app);
    assert_eq!(stored_row(app.tmp.path(), &remove), None);
    assert_eq!(
        stored_row(app.tmp.path(), &keep),
        Some((1, Some(2), Some(3)))
    );
}

#[test]
fn book_resume_meter_new_reload_replaces_old_receiver() {
    let dir = tempfile::tempdir().unwrap();
    let writer = BookResumeWriter::spawn_at(dir.path().join("book_resume.db")).unwrap();
    let book = PathBuf::from("C:/books/reload.zip");
    assert!(writer.record(&book, 0, meter(1, 3)));
    let mut cache = BookResumeMeters::default();
    cache.reload(&writer);
    writer_ack(&writer);
    assert!(writer.record(&book, 2, meter(3, 3)));
    cache.reload(&writer);
    writer_ack(&writer);
    assert!(cache.poll().unwrap().is_ok());
    assert_eq!(cache.get(&book), meter(3, 3));
    assert_eq!(cache.count(), 1);
}

#[test]
fn book_resume_meter_reload_preserves_latest_pending_resume_source() {
    let mut app = setup_app();
    settle(&mut app);
    let book = app.tmp.path().join("pending-book.zip");
    let resume = app.book_resume_writer.as_ref().unwrap().pause_for_test();
    let value = meter(8, 10);
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
        Some((7, Some(8), Some(10)))
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
    let value = meter(2, 3);
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
    let value = meter(2, 3);
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
    app.converted_archive_cache_paths.insert(
        crate::path_key::normalize_keep_drive(&sibling),
        super::ConvertedArchiveSourceState::Direct(sibling.clone()),
    );
    for idx in 0..app.items.len() {
        assert_eq!(
            app.thumbnail_book_resume_meter(idx),
            if idx < 3 || idx == 8 { value } else { None }
        );
    }
    app.settings.thumb_show_resume_meter = false;
    assert_eq!(app.thumbnail_book_resume_meter(0), None);
    app.settings.thumb_show_resume_meter = true;
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
    app.persist_book_resume(old.clone(), 0, meter(1, 5));
    settle(&mut app);
    let resume = app.book_resume_writer.as_ref().unwrap().pause_for_test();
    app.persist_book_resume(old.clone(), 3, meter(4, 5));
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
        Some((0, Some(1), Some(5))),
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
    assert_eq!(stored_row(&dir, &new), Some((3, Some(4), Some(5))));
    assert_eq!(app.book_resume_meters.get(&old), None);
    assert_eq!(app.book_resume_meters.get(&new), meter(4, 5));
}

#[test]
fn book_resume_meter_purge_retry_waits_for_unprocessed_record() {
    let mut app = setup_app();
    settle(&mut app);
    let dir = app.tmp.path().to_path_buf();
    app.rename_migration_data_dir_override = Some(dir.clone());
    let deleted = dir.join("deleted.zip");
    let sibling = dir.join("deleted.zip2");
    app.persist_book_resume(deleted.clone(), 0, meter(1, 5));
    app.persist_book_resume(sibling.clone(), 1, meter(2, 4));
    settle(&mut app);
    assert!(crate::metadata_cleanup::journal_failed_delete_purge(
        &dir,
        std::slice::from_ref(&deleted),
        &[]
    ));
    let resume = app.book_resume_writer.as_ref().unwrap().pause_for_test();
    app.persist_book_resume(deleted.clone(), 3, meter(4, 5));
    app.delete_purge_retry_needed = true;
    app.last_input_at = None;
    let ctx = egui::Context::default();
    app.poll_delete_purge_retry(&ctx);
    assert!(app.delete_purge_retry_pending.is_none());
    assert!(app.delete_purge_retry_needed);
    assert_eq!(stored_row(&dir, &deleted), Some((0, Some(1), Some(5))));

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
    assert_eq!(stored_row(&dir, &sibling), Some((1, Some(2), Some(4))));
    assert_eq!(app.book_resume_meters.get(&deleted), None);
    assert_eq!(app.book_resume_meters.get(&sibling), meter(2, 4));
}

#[test]
fn book_resume_meter_converted_archives_use_only_resolved_read_source() {
    use super::ConvertedArchiveSourceState as Source;
    use crate::archive_converter::ArchiveFormat;

    let mut app = setup_app();
    settle(&mut app);
    let direct_value = meter(2, 5);
    let cached_value = meter(4, 5);
    for ext in ["rar", "cbr", "7z", "cb7", "lzh", "lha"] {
        let source = app.tmp.path().join(format!("book.{ext}"));
        let cache = crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), &source);
        app.persist_book_resume(source.clone(), 1, direct_value);
        app.persist_book_resume(cache.clone(), 3, cached_value);
        set_pages(
            &mut app,
            vec![GridItem::ConvertibleArchive {
                path: source.clone(),
                format: ArchiveFormat::from_extension(ext).unwrap(),
            }],
        );
        let key = crate::path_key::normalize_keep_drive(&source);
        app.converted_archive_cache_paths.clear();
        assert_eq!(app.thumbnail_book_resume_meter(0), None, "unresolved {ext}");
        for state in [
            Source::Pending,
            Source::Unavailable {
                logical_source: None,
            },
        ] {
            app.converted_archive_cache_paths.insert(key.clone(), state);
            assert_eq!(app.thumbnail_book_resume_meter(0), None, "invalid {ext}");
        }
        app.converted_archive_cache_paths.insert(
            key.clone(),
            Source::CachedZip {
                logical_source: source.clone(),
                path: cache.clone(),
            },
        );
        assert_eq!(
            app.thumbnail_book_resume_meter(0),
            cached_value,
            "cached {ext}"
        );
        assert_eq!(
            app.thumbnail_resume_meter(0),
            cached_value.map(ReadingMeterValue::fraction)
        );
        app.converted_archive_cache_paths.insert(
            key.clone(),
            Source::Unavailable {
                logical_source: Some(source.clone()),
            },
        );
        assert_eq!(
            app.thumbnail_book_resume_meter(0),
            cached_value,
            "no cache {ext}"
        );
        app.settings.thumb_show_resume_meter = false;
        assert_eq!(app.thumbnail_book_resume_meter(0), None);
        app.settings.thumb_show_resume_meter = true;
        // A cache invalidation / list reload must never fall back to the source row.
        app.initialize_converted_archive_cache_paths();
        assert_eq!(app.thumbnail_book_resume_meter(0), None, "reloaded {ext}");
        if matches!(ext, "rar" | "cbr") {
            app.converted_archive_cache_paths
                .insert(key, Source::Direct(source));
            assert_eq!(
                app.thumbnail_book_resume_meter(0),
                direct_value,
                "direct {ext}"
            );
        }
    }
}

fn assert_converted_thumbnail_read_path(app: &AppTestEnv, idx: usize, expected: Option<&Path>) {
    let request = super::make_load_request(
        &app.items[idx],
        idx,
        0,
        0,
        false,
        None,
        None,
        app.settings.folder_thumb_depth,
        &Default::default(),
        &app.converted_archive_cache_paths,
        None,
        None,
        None,
        None,
        false,
    );
    assert_eq!(
        request.as_ref().map(|request| request.path.as_path()),
        expected
    );
}

#[test]
fn book_resume_meter_split_rar_hides_later_volume_without_changing_thumbnail_sources() {
    use super::ConvertedArchiveSourceState as Source;
    let mut app = setup_app();
    settle(&mut app);
    for ext in ["rar", "cbr"] {
        let first = app.tmp.path().join(format!("book.part1.{ext}"));
        let next = app.tmp.path().join(format!("book.part2.{ext}"));
        let cache = crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), &first);
        let next_cache = crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), &next);
        let direct_value = meter(3, 5);
        let cached_value = meter(4, 5);
        app.persist_book_resume(first.clone(), 2, direct_value);
        app.persist_book_resume(next.clone(), 0, meter(1, 5));
        app.persist_book_resume(cache.clone(), 3, cached_value);
        app.persist_book_resume(next_cache, 1, meter(2, 5));
        set_pages(
            &mut app,
            [&first, &next]
                .into_iter()
                .map(|path| GridItem::ConvertibleArchive {
                    path: path.clone(),
                    format: crate::archive_converter::ArchiveFormat::Rar,
                })
                .collect(),
        );
        let first_key = crate::path_key::normalize_keep_drive(&first);
        let next_key = crate::path_key::normalize_keep_drive(&next);
        for (state, first_value, thumbnail_path) in [
            (
                Source::Direct(first.clone()),
                direct_value,
                Some(first.as_path()),
            ),
            (
                Source::CachedZip {
                    logical_source: first.clone(),
                    path: cache.clone(),
                },
                cached_value,
                Some(cache.as_path()),
            ),
            (
                Source::Unavailable {
                    logical_source: Some(first.clone()),
                },
                cached_value,
                None,
            ),
        ] {
            app.converted_archive_cache_paths.clear();
            app.converted_archive_cache_paths
                .insert(first_key.clone(), state.clone());
            app.converted_archive_cache_paths.insert(
                next_key.clone(),
                Source::Rar {
                    volume: crate::rar_loader::RarVolumeProof::Subsequent {
                        first: first.clone(),
                    },
                    source: Box::new(state),
                },
            );
            let thumbnail_sources = app.converted_archive_cache_paths.clone();
            assert_eq!(app.thumbnail_book_resume_meter(0), first_value);
            assert_eq!(app.thumbnail_book_resume_meter(1), None, "later {ext}");
            assert_eq!(app.thumbnail_resume_meter(1), None);
            for idx in [0, 1] {
                assert_converted_thumbnail_read_path(&app, idx, thumbnail_path);
            }
            app.settings.thumb_show_resume_meter = false;
            assert_eq!(app.thumbnail_book_resume_meter(0), None);
            assert_eq!(app.thumbnail_book_resume_meter(1), None);
            app.settings.thumb_show_resume_meter = true;
            assert_eq!(app.converted_archive_cache_paths, thumbnail_sources);
        }
        for state in [
            Source::Pending,
            Source::Unavailable {
                logical_source: None,
            },
        ] {
            app.converted_archive_cache_paths
                .insert(first_key.clone(), state.clone());
            app.converted_archive_cache_paths
                .insert(next_key.clone(), state);
            assert_eq!(app.thumbnail_book_resume_meter(0), None);
            assert_eq!(app.thumbnail_book_resume_meter(1), None);
        }
        // A filename alone does not prove a later volume: retain its own released row
        // when the worker's logical source equals the cell path.
        app.converted_archive_cache_paths
            .insert(next_key, Source::Direct(next.clone()));
        assert_eq!(app.thumbnail_book_resume_meter(1), meter(1, 5));
    }
}

#[test]
fn book_resume_meter_archive_cache_deletion_completion_rechecks_sources() {
    use super::ConvertedArchiveSourceState as Source;
    use super::{ConvertedArchiveCachePathsMsg, ConvertedArchiveCachePathsPending};
    use crate::cache_maintenance::{ArchiveMaintTask, spawn_archive};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    for action in ["selected", "missing", "all"] {
        let mut app = setup_app();
        settle(&mut app);
        let source = app.tmp.path().join("book.7z");
        let direct = app.tmp.path().join("direct.rar");
        std::fs::write(&source, b"archive").unwrap();
        let metadata = std::fs::metadata(&source).unwrap();
        let stamp = (
            crate::ui_helpers::mtime_secs(&metadata),
            metadata.len() as i64,
        );
        let db = app.archive_cache_db.as_ref().unwrap().clone();
        let cached = db.reserve_cache_zip_path(&source).unwrap();
        std::fs::write(&cached, b"cached archive").unwrap();
        db.record(
            &source,
            stamp.0,
            stamp.1,
            crate::archive_converter::ArchiveFormat::SevenZ,
            &cached,
            14,
            5,
            false,
        )
        .unwrap();
        app.persist_book_resume(cached.clone(), 2, meter(3, 5));
        app.persist_book_resume(direct.clone(), 1, meter(2, 5));
        set_pages(
            &mut app,
            vec![
                GridItem::ConvertibleArchive {
                    path: source.clone(),
                    format: crate::archive_converter::ArchiveFormat::SevenZ,
                },
                GridItem::ConvertibleArchive {
                    path: direct.clone(),
                    format: crate::archive_converter::ArchiveFormat::Rar,
                },
            ],
        );
        app.image_metas = vec![Some(stamp), None];
        let key = crate::path_key::normalize_keep_drive(&source);
        let direct_key = crate::path_key::normalize_keep_drive(&direct);
        app.converted_archive_cache_paths.insert(
            key.clone(),
            Source::CachedZip {
                logical_source: source.clone(),
                path: cached.clone(),
            },
        );
        app.converted_archive_cache_paths
            .insert(direct_key.clone(), Source::Direct(direct.clone()));
        assert_eq!(app.thumbnail_book_resume_meter(0), meter(3, 5));

        // A pre-delete result already queued in the source owner's old batch must be discarded.
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        app.converted_archive_cache_paths_pending = Some(ConvertedArchiveCachePathsPending {
            generation: app.items_generation,
            cascade_depth: app.settings.folder_thumb_depth as usize,
            cancel: cancel.clone(),
            desired_indices: Arc::new(std::sync::RwLock::new(std::collections::HashSet::from([0]))),
            rx,
            pin_archive_dependencies: Default::default(),
        });
        tx.send(ConvertedArchiveCachePathsMsg::Resolved {
            archive_key: key.clone(),
            state: Source::CachedZip {
                logical_source: source.clone(),
                path: cached.clone(),
            },
            idx: 0,
            ordinal: 1,
            elapsed_ms: 0.0,
            input_seq: 0,
        })
        .unwrap();
        let task = match action {
            "selected" => ArchiveMaintTask::DeleteSelected {
                src_paths: vec![source.clone()],
            },
            "missing" => {
                std::fs::remove_file(&source).unwrap();
                ArchiveMaintTask::DeleteMissing
            }
            _ => ArchiveMaintTask::DeleteAll,
        };
        app.archive_cache_manager_result = None;
        app.archive_cache_maint_pending = Some(spawn_archive(task, db.clone()));
        let deadline = Instant::now() + WORKER_TIMEOUT;
        while app.archive_cache_manager_result.is_none() {
            app.poll_archive_cache_maint_pending();
            assert!(
                Instant::now() < deadline,
                "{action}: maintenance did not finish"
            );
            std::thread::yield_now();
        }
        assert!(
            !cached.exists(),
            "{action}: the real deletion worker must remove the ZIP"
        );
        assert_eq!(
            app.thumbnail_book_resume_meter(0),
            None,
            "{action}: deleted cache bar"
        );
        assert!(
            cancel.load(Ordering::Relaxed),
            "{action}: discard stale batch"
        );
        app.poll_converted_archive_cache_paths(&egui::Context::default());
        assert_eq!(
            app.converted_archive_cache_paths.get(&key),
            Some(&Source::Pending)
        );
        assert_eq!(
            app.thumbnail_book_resume_meter(1),
            meter(2, 5),
            "Direct is unaffected"
        );
        assert_eq!(
            app.book_resume_meters.get(&cached),
            meter(3, 5),
            "saved record is retained"
        );
        let scope = std::collections::HashSet::from([0]);
        app.start_converted_archive_cache_paths_refresh(&scope, (0, 1), (0, 1));
        let deadline = Instant::now() + WORKER_TIMEOUT;
        while app.converted_archive_cache_paths_pending.is_some() {
            app.poll_converted_archive_cache_paths(&egui::Context::default());
            assert!(
                Instant::now() < deadline,
                "{action}: source recheck did not finish"
            );
            std::thread::yield_now();
        }
        assert_eq!(
            app.converted_archive_cache_paths.get(&key),
            Some(&Source::Unavailable {
                logical_source: Some(source.clone())
            })
        );
        assert_eq!(app.thumbnail_book_resume_meter(0), meter(3, 5));
    }
}

#[test]
fn book_resume_meter_smart_parent_cache_delete_backspace_rechecks_stashed_sources() {
    use super::ConvertedArchiveSourceState as Source;
    use crate::cache_maintenance::{ArchiveMaintTask, spawn_archive};

    let mut app = setup_app();
    settle(&mut app);
    app.active_quick_folder_slot = None;
    let root = app.tmp.path().join("smart-meter-root");
    let child = root.join("child");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(child.join("page.jpg"), []).unwrap();
    let source = root.join("book.7z");
    std::fs::write(&source, b"archive").unwrap();
    let metadata = std::fs::metadata(&source).unwrap();
    let db = app.archive_cache_db.as_ref().unwrap().clone();
    let cached = db.reserve_cache_zip_path(&source).unwrap();
    std::fs::write(&cached, b"cached archive").unwrap();
    db.record(
        &source,
        crate::ui_helpers::mtime_secs(&metadata),
        metadata.len() as i64,
        crate::archive_converter::ArchiveFormat::SevenZ,
        &cached,
        14,
        5,
        false,
    )
    .unwrap();
    app.persist_book_resume(cached.clone(), 2, meter(3, 5));
    let mut definition = crate::settings::SmartFolderDefinition::new("Smart meter");
    definition.rules.push(crate::settings::SmartFolderRule::new(
        root,
        true,
        Default::default(),
    ));
    let id = definition.id;
    app.settings.smart_folders = vec![definition];
    let ctx = egui::Context::default();
    app.open_smart_folder(id, false);
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while !app.items_are_smart_folder_view
        || app.smart_folder_transition.is_some()
        || app.smart_folder_prepare_pending.is_some()
    {
        app.poll_smart_folder(&ctx);
        assert!(Instant::now() < deadline, "smart root did not finish");
        std::thread::yield_now();
    }
    let key = crate::path_key::normalize_keep_drive(&source);
    let index = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(&source))
        .unwrap();
    let scope = std::collections::HashSet::from([index]);
    app.start_converted_archive_cache_paths_refresh(&scope, (index, index + 1), (index, index + 1));
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while app.converted_archive_cache_paths_pending.is_some() {
        app.poll_converted_archive_cache_paths(&ctx);
        assert!(
            Instant::now() < deadline,
            "smart root source did not finish"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        app.converted_archive_cache_paths.get(&key),
        Some(&Source::CachedZip {
            logical_source: source.clone(),
            path: cached.clone()
        })
    );
    // Synthetic Smart roots intentionally hide meters; the source map also serves thumbnails.
    assert_eq!(app.thumbnail_book_resume_meter(index), None);

    // Production child adoption moves the parent grid, including the resolved source map.
    app.open_staged_smart_folder_and_wait(&ctx, &child);
    assert!(!app.converted_archive_cache_paths.contains_key(&key));
    app.archive_cache_manager_result = None;
    app.archive_cache_maint_pending = Some(spawn_archive(ArchiveMaintTask::DeleteAll, db));
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while app.archive_cache_manager_result.is_none() {
        app.poll_archive_cache_maint_pending();
        assert!(Instant::now() < deadline, "cache delete did not finish");
        std::thread::yield_now();
    }
    assert!(!cached.exists());
    let parent = app
        .resolve_grid_parent_nav()
        .expect("Backspace parent route");
    assert!(app.apply_fullscreen_close_nav_immediate(parent));
    assert!(app.items_are_smart_folder_view);
    assert!(
        app.smart_folder_transition.is_none(),
        "restore the moved grid, not a rescan"
    );
    let index = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(&source))
        .unwrap();
    assert_eq!(app.thumbnail_book_resume_meter(index), None);
    assert_eq!(
        app.converted_archive_cache_paths.get(&key),
        Some(&Source::Pending)
    );
    let scope = std::collections::HashSet::from([index]);
    app.start_converted_archive_cache_paths_refresh(&scope, (index, index + 1), (index, index + 1));
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while app.converted_archive_cache_paths_pending.is_some() {
        app.poll_converted_archive_cache_paths(&ctx);
        assert!(
            Instant::now() < deadline,
            "restored parent source did not finish"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        app.converted_archive_cache_paths.get(&key),
        Some(&Source::Unavailable {
            logical_source: Some(source.clone())
        })
    );
    assert_eq!(app.book_resume_meters.get(&cached), meter(3, 5));

    // Sort-only prepare reuses the original Arc metadata, whose cache resolution predates
    // deletion. Its real prepare/adoption path must not republish that terminal CachedZip.
    assert!(app.reprepare_current_smart_folder_for_sort());
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while app.smart_folder_prepare_pending.is_some() {
        app.poll_smart_folder(&ctx);
        assert!(
            Instant::now() < deadline,
            "reused smart metadata did not finish"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        app.converted_archive_cache_paths.get(&key),
        Some(&Source::Pending)
    );
    let index = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(&source))
        .unwrap();
    let scope = std::collections::HashSet::from([index]);
    app.start_converted_archive_cache_paths_refresh(&scope, (index, index + 1), (index, index + 1));
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while app.converted_archive_cache_paths_pending.is_some() {
        app.poll_converted_archive_cache_paths(&ctx);
        assert!(
            Instant::now() < deadline,
            "reused source recheck did not finish"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        app.converted_archive_cache_paths.get(&key),
        Some(&Source::Unavailable {
            logical_source: Some(source.clone())
        })
    );
}

#[test]
fn book_resume_meter_cache_delete_invalidates_parked_source_owner_without_mounting() {
    use super::ConvertedArchiveSourceState as Source;
    use super::{ConvertedArchiveCachePathsMsg, ConvertedArchiveCachePathsPending};
    use crate::cache_maintenance::{ArchiveMaintTask, spawn_archive};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    let mut app = setup_app();
    settle(&mut app);
    let source = app.tmp.path().join("parked.7z");
    let cached = app.tmp.path().join("parked.zip");
    let direct = app.tmp.path().join("direct.rar");
    let key = crate::path_key::normalize_keep_drive(&source);
    let direct_key = crate::path_key::normalize_keep_drive(&direct);
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let parked = app.build_window_context_for_test(1350, |owner| {
        owner.converted_archive_cache_paths.insert(
            key.clone(),
            Source::CachedZip {
                logical_source: source.clone(),
                path: cached.clone(),
            },
        );
        owner
            .converted_archive_cache_paths
            .insert(direct_key.clone(), Source::Direct(direct.clone()));
        owner.fs_pan = egui::vec2(13.0, 50.0);
        owner.converted_archive_cache_paths_pending = Some(ConvertedArchiveCachePathsPending {
            generation: owner.items_generation,
            cascade_depth: owner.settings.folder_thumb_depth as usize,
            cancel: cancel.clone(),
            desired_indices: Default::default(),
            rx,
            pin_archive_dependencies: Default::default(),
        });
        tx.send(ConvertedArchiveCachePathsMsg::Resolved {
            archive_key: key.clone(),
            state: Source::CachedZip {
                logical_source: source.clone(),
                path: cached.clone(),
            },
            idx: 0,
            ordinal: 1,
            elapsed_ms: 0.0,
            input_seq: 0,
        })
        .unwrap();
    });
    let mounted = app.mounted_viewer_context_id();
    let generation = app.items_generation;
    let db = app.archive_cache_db.as_ref().unwrap().clone();
    app.archive_cache_manager_result = None;
    app.archive_cache_maint_pending = Some(spawn_archive(ArchiveMaintTask::DeleteAll, db));
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while app.archive_cache_manager_result.is_none() {
        app.poll_archive_cache_maint_pending();
        assert!(Instant::now() < deadline, "cache delete did not finish");
        std::thread::yield_now();
    }
    assert_eq!(app.mounted_viewer_context_id(), mounted);
    assert_eq!(app.items_generation, generation);
    assert!(
        cancel.load(Ordering::Relaxed),
        "parked batch is cancelled at deletion"
    );
    app.with_window_context_for_test(parked, |owner| {
        owner.poll_converted_archive_cache_paths(&egui::Context::default());
        assert_eq!(
            owner.converted_archive_cache_paths.get(&key),
            Some(&Source::Pending)
        );
        assert_eq!(
            owner.converted_archive_cache_paths.get(&direct_key),
            Some(&Source::Direct(direct.clone()))
        );
        assert!(owner.converted_archive_cache_paths_pending.is_none());
        assert_eq!(owner.fs_pan, egui::vec2(13.0, 50.0));
    });
}

#[test]
fn book_resume_meter_archive_cache_rows_and_error_do_not_invalidate_sources() {
    use super::ConvertedArchiveSourceState as Source;
    use crate::cache_maintenance::{ArchiveMaintPending, ArchiveMaintResult, ArchiveMaintTask};

    let mut app = setup_app();
    settle(&mut app);
    let source = app.tmp.path().join("book.7z");
    let cached = crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), &source);
    app.persist_book_resume(cached.clone(), 1, meter(2, 5));
    set_pages(
        &mut app,
        vec![GridItem::ConvertibleArchive {
            path: source.clone(),
            format: crate::archive_converter::ArchiveFormat::SevenZ,
        }],
    );
    let key = crate::path_key::normalize_keep_drive(&source);
    app.converted_archive_cache_paths.insert(
        key.clone(),
        Source::CachedZip {
            logical_source: source.clone(),
            path: cached.clone(),
        },
    );
    for result in [
        ArchiveMaintResult::Rows {
            entries: vec![],
            total_bytes: 0,
        },
        ArchiveMaintResult::Error("deletion failed".into()),
    ] {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(result).unwrap();
        app.archive_cache_maint_pending = Some(ArchiveMaintPending {
            task: ArchiveMaintTask::LoadRows,
            rx,
        });
        app.poll_archive_cache_maint_pending();
        assert_eq!(
            app.converted_archive_cache_paths.get(&key),
            Some(&Source::CachedZip {
                logical_source: source.clone(),
                path: cached.clone()
            })
        );
        assert_eq!(app.thumbnail_book_resume_meter(0), meter(2, 5));
    }
}

fn refresh_converted_source(app: &mut AppTestEnv, idx: usize) {
    let scope = std::collections::HashSet::from([idx]);
    app.start_converted_archive_cache_paths_refresh(&scope, (idx, idx + 1), (idx, idx + 1));
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while app.converted_archive_cache_paths_pending.is_some() {
        app.poll_converted_archive_cache_paths(&egui::Context::default());
        assert!(Instant::now() < deadline, "source worker did not finish");
        std::thread::yield_now();
    }
}

fn delete_converted_cache(app: &mut AppTestEnv, source: &Path) {
    use crate::cache_maintenance::{ArchiveMaintTask, spawn_archive};
    app.archive_cache_manager_result = None;
    app.archive_cache_maint_pending = Some(spawn_archive(
        ArchiveMaintTask::DeleteSelected {
            src_paths: vec![source.to_path_buf()],
        },
        app.archive_cache_db.as_ref().unwrap().clone(),
    ));
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while app.archive_cache_manager_result.is_none() {
        app.poll_archive_cache_maint_pending();
        assert!(Instant::now() < deadline, "cache deletion did not finish");
        std::thread::yield_now();
    }
}

fn convert_and_record_cache(
    app: &AppTestEnv,
    source: &Path,
    format: crate::archive_converter::ArchiveFormat,
) -> PathBuf {
    let db = app.archive_cache_db.as_ref().unwrap();
    let cached = db.reserve_cache_zip_path(source).unwrap();
    crate::archive_converter::convert_to_zip(
        source,
        &cached,
        format,
        &std::sync::atomic::AtomicBool::new(false),
        None,
    )
    .unwrap();
    let meta = std::fs::metadata(source).unwrap();
    let pages = crate::zip_loader::enumerate_image_entries(&cached)
        .unwrap()
        .len();
    db.record(
        source,
        crate::ui_helpers::mtime_secs(&meta),
        meta.len() as i64,
        format,
        &cached,
        std::fs::metadata(&cached).unwrap().len() as i64,
        pages.try_into().unwrap(),
        false,
    )
    .unwrap();
    cached
}

fn install_converted_zip(app: &mut AppTestEnv, source: &Path, cached: &Path) {
    let enumeration = crate::zip_loader::enumerate_image_entries_detailed(cached).unwrap();
    app.settings.sidecar_backup_enabled = false;
    app.settings.tag_sidecar_backup_enabled = false;
    app.load_zip_as_folder_prepared_with_logical_source(
        cached.to_path_buf(),
        enumeration,
        Some(source),
        super::StartupListIntent::ExplicitList,
    );
    assert_eq!(app.current_folder.as_deref(), Some(cached));
}

#[test]
fn book_resume_meter_copied_data_dir_uses_existing_cache_save_restore_key() {
    use super::tests::phase_c_support::setup_paused_similar_app_with_fixture;
    use crate::archive_converter::ArchiveFormat;

    let mut old_app = setup_app();
    settle(&mut old_app);
    let source = old_app.tmp.path().join("copied-profile.7z");
    let mut writer = sevenz_rust2::ArchiveWriter::create(&source).unwrap();
    for name in ["01.jpg", "02.jpg", "03.jpg"] {
        writer
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file(name),
                Some(std::io::Cursor::new(vec![1u8; 3])),
            )
            .unwrap();
    }
    writer.finish().unwrap();
    let cached = convert_and_record_cache(&old_app, &source, ArchiveFormat::SevenZ);
    install_converted_zip(&mut old_app, &source, &cached);
    old_app.record_book_resume(1);
    settle(&mut old_app);

    // Close every old-profile DB before copying; retain its directory and valid ZIP,
    // as the documented migration procedure does until the copied profile is verified.
    let old_profile = std::mem::replace(&mut old_app.tmp, tempfile::tempdir().unwrap());
    drop(old_app);
    let mut app =
        setup_paused_similar_app_with_fixture(crate::settings::Settings::default(), |new_dir| {
            for name in ["archive_cache.db", "book_resume.db"] {
                std::fs::copy(old_profile.path().join(name), new_dir.join(name)).unwrap();
            }
        });
    settle(&mut app);
    let computed = crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), &source);
    assert_ne!(computed, cached);
    let meta = std::fs::metadata(&source).unwrap();
    let db = app.archive_cache_db.as_ref().unwrap();
    let stamp = crate::ui_helpers::mtime_secs(&meta);
    assert_eq!(
        db.peek(&source, stamp, meta.len() as i64),
        Some(cached.clone())
    );
    assert_eq!(
        db.lookup(&source, stamp, meta.len() as i64),
        Some(cached.clone())
    );

    app.load_folder(source.parent().unwrap().to_path_buf());
    let idx = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(&source))
        .unwrap();
    refresh_converted_source(&mut app, idx);
    assert_eq!(app.thumbnail_book_resume_meter(idx), meter(2, 3));
    install_converted_zip(&mut app, &source, &cached);
    assert_eq!(app.resume_page_for_container(), Some(1));
    app.record_book_resume(2);
    settle(&mut app);
    assert_eq!(
        stored_row(app.tmp.path(), &cached),
        Some((2, Some(3), Some(3)))
    );
    assert_eq!(stored_row(app.tmp.path(), &computed), None);

    app.load_folder(source.parent().unwrap().to_path_buf());
    let idx = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(&source))
        .unwrap();
    refresh_converted_source(&mut app, idx);
    assert_eq!(app.thumbnail_book_resume_meter(idx), meter(3, 3));
    // No aliases/migration machinery: after deleting the old ZIP, the missing-cache
    // state computes the new profile's key. The old released resume row survives.
    delete_converted_cache(&mut app, &source);
    refresh_converted_source(&mut app, idx);
    assert_eq!(app.thumbnail_book_resume_meter(idx), None);
    assert_eq!(
        stored_row(app.tmp.path(), &cached),
        Some((2, Some(3), Some(3)))
    );
}

#[test]
fn book_resume_meter_deleted_cache_reconversion_and_source_change_resume_same_key() {
    use super::ConvertedArchiveSourceState as Source;
    use crate::archive_converter::ArchiveFormat;
    let mut app = setup_app();
    settle(&mut app);
    let source = app.tmp.path().join("resume.7z");
    let write_source = |extra: bool| {
        let mut writer = sevenz_rust2::ArchiveWriter::create(&source).unwrap();
        for name in ["01.jpg", "02.jpg", "03.jpg"] {
            writer
                .push_archive_entry(
                    sevenz_rust2::ArchiveEntry::new_file(name),
                    Some(std::io::Cursor::new(vec![1u8; if extra { 19 } else { 3 }])),
                )
                .unwrap();
        }
        writer.finish().unwrap();
    };
    write_source(false);
    let cached = convert_and_record_cache(&app, &source, ArchiveFormat::SevenZ);
    install_converted_zip(&mut app, &source, &cached);
    app.record_book_resume(1);
    settle(&mut app);
    assert_eq!(
        stored_row(app.tmp.path(), &cached),
        Some((1, Some(2), Some(3)))
    );

    app.load_folder(source.parent().unwrap().to_path_buf());
    let idx = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(&source))
        .unwrap();
    refresh_converted_source(&mut app, idx);
    assert_eq!(app.thumbnail_book_resume_meter(idx), meter(2, 3));
    delete_converted_cache(&mut app, &source);
    assert!(!cached.exists());
    assert_eq!(
        app.thumbnail_book_resume_meter(idx),
        None,
        "Pending is hidden"
    );
    refresh_converted_source(&mut app, idx);
    assert_eq!(app.thumbnail_book_resume_meter(idx), meter(2, 3));
    assert!(
        matches!(app.converted_archive_cache_paths.values().next(), Some(Source::Unavailable { logical_source: Some(path) }) if path == &source)
    );
    assert_eq!(
        convert_and_record_cache(&app, &source, ArchiveFormat::SevenZ),
        cached
    );
    install_converted_zip(&mut app, &source, &cached);
    assert_eq!(app.resume_page_for_container(), Some(1));

    // Reachable cache lookup stamp invalidation must not delete released resume data either.
    write_source(true);
    let meta = std::fs::metadata(&source).unwrap();
    let db = app.archive_cache_db.as_ref().unwrap();
    assert!(
        db.lookup(
            &source,
            crate::ui_helpers::mtime_secs(&meta),
            meta.len() as i64
        )
        .is_none()
    );
    assert_eq!(
        stored_row(app.tmp.path(), &cached),
        Some((1, Some(2), Some(3)))
    );
    assert_eq!(
        convert_and_record_cache(&app, &source, ArchiveFormat::SevenZ),
        cached
    );
    install_converted_zip(&mut app, &source, &cached);
    assert_eq!(app.resume_page_for_container(), Some(1));
}

#[test]
fn book_resume_meter_multipart_direct_worker_keeps_later_thumbnail_but_hides_meter() {
    use super::ConvertedArchiveSourceState as Source;
    use crate::archive_converter::ArchiveFormat;
    let mut app = setup_app();
    settle(&mut app);
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/archives/rar-multipart-filename-regression/real-split-control");
    let paths: Vec<_> = [1, 2]
        .into_iter()
        .map(|part| {
            let name = format!("real-split-control.part{part}.rar");
            let path = app.tmp.path().join(&name);
            std::fs::copy(fixture.join(name), &path).unwrap();
            path
        })
        .collect();
    let first = &paths[0];
    let next = &paths[1];
    let inspection = crate::rar_loader::inspect_for_direct_read(next).unwrap();
    assert_eq!(inspection.resolved_path, *first);
    assert_eq!(
        inspection.decision,
        crate::rar_loader::RarDirectReadDecision::Direct
    );
    app.persist_book_resume(first.clone(), 0, meter(1, 1));
    app.persist_book_resume(next.clone(), 0, meter(1, 5));
    let next_cache = crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), next);
    app.persist_book_resume(next_cache, 0, meter(1, 5));
    app.install_new_items(
        paths
            .iter()
            .map(|path| GridItem::ConvertibleArchive {
                path: path.clone(),
                format: ArchiveFormat::Rar,
            })
            .collect(),
        paths
            .iter()
            .map(|path| {
                let meta = std::fs::metadata(path).unwrap();
                Some((crate::ui_helpers::mtime_secs(&meta), meta.len() as i64))
            })
            .collect(),
    );
    assert_eq!(
        app.thumbnail_book_resume_meter(0),
        None,
        "Pending is hidden"
    );
    assert_eq!(
        app.thumbnail_book_resume_meter(1),
        None,
        "Pending is hidden"
    );
    for idx in [0, 1] {
        refresh_converted_source(&mut app, idx);
        assert_eq!(
            app.converted_archive_cache_paths
                .get(&crate::path_key::normalize_keep_drive(&paths[idx]))
                .map(super::ConvertedArchiveSourceState::read_source),
            Some(&Source::Direct(first.clone()))
        );
        assert_converted_thumbnail_read_path(&app, idx, Some(first));
    }
    let thumbnail_sources = app.converted_archive_cache_paths.clone();
    assert_eq!(app.thumbnail_book_resume_meter(0), meter(1, 1));
    assert_eq!(app.thumbnail_book_resume_meter(1), None);
    assert_eq!(app.thumbnail_resume_meter(1), None);
    app.settings.thumb_show_resume_meter = false;
    assert_eq!(app.thumbnail_book_resume_meter(0), None);
    assert_eq!(app.thumbnail_book_resume_meter(1), None);
    assert_eq!(app.converted_archive_cache_paths, thumbnail_sources);
}

#[test]
fn book_resume_meter_deleted_multipart_solid_rar_keeps_first_meter_only() {
    use crate::archive_converter::ArchiveFormat;
    let mut app = setup_app();
    settle(&mut app);
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/archives/rar-multipart-filename-regression/real-split-control");
    // Declare the existing valid RAR5 split fixture solid in its main header, retaining
    // the compressed entries and real volume headers. Recompute each header CRC.
    // This exercises the production solid-conversion branch without an external packer.
    let mut paths = Vec::new();
    for part in [1, 2] {
        let name = format!("real-split-control.part{part}.rar");
        let mut bytes = std::fs::read(fixture.join(&name)).unwrap();
        assert_eq!(&bytes[..8], b"Rar!\x1a\x07\x01\x00");
        assert_eq!(bytes[13], 1, "main header");
        bytes[16] |= 4; // RAR5 archive flag: solid.
        let end = 13 + bytes[12] as usize;
        let mut crc = u32::MAX;
        for byte in &bytes[12..end] {
            crc ^= *byte as u32;
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
            }
        }
        bytes[8..12].copy_from_slice(&(!crc).to_le_bytes());
        let path = app.tmp.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        paths.push(path);
    }
    let first = &paths[0];
    let next = &paths[1];
    let inspection = crate::rar_loader::inspect_for_direct_read(next).unwrap();
    assert_eq!(inspection.resolved_path, *first);
    assert_eq!(
        inspection.decision,
        crate::rar_loader::RarDirectReadDecision::Solid
    );
    let cached = convert_and_record_cache(&app, first, ArchiveFormat::Rar);
    app.persist_book_resume(cached.clone(), 0, meter(1, 1));
    // An unrelated row at the subsequent-volume would-be cache key must not win.
    let next_cache = crate::archive_cache::cache_zip_path_for_data_dir(app.tmp.path(), next);
    app.persist_book_resume(next_cache, 0, meter(1, 5));
    app.install_new_items(
        paths
            .iter()
            .map(|path| GridItem::ConvertibleArchive {
                path: path.clone(),
                format: ArchiveFormat::Rar,
            })
            .collect(),
        paths
            .iter()
            .map(|path| {
                let meta = std::fs::metadata(path).unwrap();
                Some((crate::ui_helpers::mtime_secs(&meta), meta.len() as i64))
            })
            .collect(),
    );
    for idx in [0, 1] {
        refresh_converted_source(&mut app, idx);
        assert_eq!(
            app.converted_archive_cache_paths
                .get(&crate::path_key::normalize_keep_drive(&paths[idx]))
                .map(super::ConvertedArchiveSourceState::read_source),
            Some(&super::ConvertedArchiveSourceState::CachedZip {
                logical_source: first.clone(),
                path: cached.clone(),
            })
        );
        assert_converted_thumbnail_read_path(&app, idx, Some(&cached));
    }
    let thumbnail_sources = app.converted_archive_cache_paths.clone();
    assert_eq!(app.thumbnail_book_resume_meter(0), meter(1, 1));
    assert_eq!(app.thumbnail_book_resume_meter(1), None);
    assert_eq!(app.converted_archive_cache_paths, thumbnail_sources);
    delete_converted_cache(&mut app, first);
    assert!(!cached.exists());
    assert_eq!(
        app.thumbnail_book_resume_meter(0),
        None,
        "Pending is hidden"
    );
    assert_eq!(
        app.thumbnail_book_resume_meter(1),
        None,
        "Pending is hidden"
    );
    for idx in [0, 1] {
        refresh_converted_source(&mut app, idx);
        assert_eq!(
            app.converted_archive_cache_paths
                .get(&crate::path_key::normalize_keep_drive(&paths[idx]))
                .map(super::ConvertedArchiveSourceState::read_source),
            Some(&super::ConvertedArchiveSourceState::Unavailable {
                logical_source: Some(first.clone())
            })
        );
        assert_converted_thumbnail_read_path(&app, idx, None);
    }
    let thumbnail_sources = app.converted_archive_cache_paths.clone();
    assert_eq!(app.thumbnail_book_resume_meter(0), meter(1, 1));
    assert_eq!(app.thumbnail_book_resume_meter(1), None);
    assert_eq!(app.thumbnail_resume_meter(1), None);
    app.settings.thumb_show_resume_meter = false;
    assert_eq!(app.thumbnail_book_resume_meter(0), None);
    assert_eq!(app.thumbnail_book_resume_meter(1), None);
    assert_eq!(app.converted_archive_cache_paths, thumbnail_sources);
    settle(&mut app);
    assert_eq!(
        stored_row(app.tmp.path(), &cached),
        Some((0, Some(1), Some(1)))
    );
}

#[test]
fn book_resume_meter_uppercase_later_rar_with_legacy_cache_is_hidden() {
    use crate::archive_converter::ArchiveFormat;
    for extension in ["RAR", "cbr", "CBR"] {
        let mut app = setup_app();
        settle(&mut app);
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/archives/rar-multipart-filename-regression/real-split-control");
        let first = app.tmp.path().join(format!("renamed.part1.{extension}"));
        let later = app.tmp.path().join(format!("renamed.part2.{extension}"));
        for (part, target) in [(1, &first), (2, &later)] {
            std::fs::copy(
                fixture.join(format!("real-split-control.part{part}.rar")),
                target,
            )
            .unwrap();
        }
        let cached = convert_and_record_cache(&app, &first, ArchiveFormat::Rar);
        let meta = std::fs::metadata(&later).unwrap();
        app.archive_cache_db
            .as_ref()
            .unwrap()
            .record(
                &later,
                crate::ui_helpers::mtime_secs(&meta),
                meta.len() as i64,
                ArchiveFormat::Rar,
                &cached,
                std::fs::metadata(&cached).unwrap().len() as i64,
                1,
                false,
            )
            .unwrap();
        app.persist_book_resume(cached.clone(), 0, meter(1, 1));
        settle(&mut app);
        app.install_new_items(
            vec![GridItem::ConvertibleArchive {
                path: later.clone(),
                format: ArchiveFormat::Rar,
            }],
            vec![Some((
                crate::ui_helpers::mtime_secs(&meta),
                meta.len() as i64,
            ))],
        );
        refresh_converted_source(&mut app, 0);
        assert_eq!(
            app.thumbnail_book_resume_meter(0),
            None,
            "confirmed subsequent {extension} is not openable"
        );
        assert!(cached.exists());
    }
}

#[test]
fn book_resume_meter_native_subsequent_proof_hides_even_when_first_path_equals_cell() {
    use super::ConvertedArchiveSourceState as Source;
    let mut app = setup_app();
    settle(&mut app);
    let clicked = app.tmp.path().join("book.part2.RAR");
    app.install_new_items(
        vec![GridItem::ConvertibleArchive {
            path: clicked.clone(),
            format: crate::archive_converter::ArchiveFormat::Rar,
        }],
        vec![None],
    );
    app.persist_book_resume(clicked.clone(), 0, meter(1, 2));
    settle(&mut app);
    let key = crate::path_key::normalize_keep_drive(&clicked);
    app.converted_archive_cache_paths.insert(
        key.clone(),
        Source::Rar {
            volume: crate::rar_loader::RarVolumeProof::Subsequent {
                first: clicked.clone(),
            },
            source: Box::new(Source::Direct(clicked.clone())),
        },
    );
    assert_eq!(app.thumbnail_book_resume_meter(0), None);
    assert_converted_thumbnail_read_path(&app, 0, Some(&clicked));
    app.converted_archive_cache_paths.insert(
        key,
        Source::Rar {
            volume: crate::rar_loader::RarVolumeProof::First,
            source: Box::new(Source::Direct(clicked)),
        },
    );
    assert_eq!(app.thumbnail_book_resume_meter(0), meter(1, 2));
}
