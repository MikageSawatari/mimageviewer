use super::*;
use crate::app::{
    RenameMigrationInFlight, RenameMigrationJournalAdmission, ThumbnailState,
    page_edit_snapshot as edits, tests::phase_c_support::setup_app,
};
use crate::book_resume_db::{PageIdentity, ReadingMeterValue};
use crate::book_resume_key::BookResumeKey;
use crate::grid_item::GridItem;
use crate::rename_key_migration::{
    self as migration,
    resume_identity_tests::{archive_pages, assert_restored, read_position, record_position},
};
use std::sync::{Arc, atomic::AtomicBool, mpsc};
use std::time::{Duration, Instant};

fn cache(data: &Path, source: &Path) -> PathBuf {
    crate::archive_cache::cache_zip_path_for_data_dir(data, source)
}

fn page_key(path: &Path) -> String {
    crate::adjustment_db::zip_entry_key(path, "02.jpg")
}

fn adopt_rename(app: &mut App, old: &Path, new: &Path, tree: bool) {
    let enumeration = if tree {
        migration::collect_converted_archive_rels(new)
    } else {
        Default::default()
    };
    assert!(!enumeration.incomplete);
    let (tx, rx) = mpsc::channel();
    app.rename_pending = Some(crate::ui_dialogs::rename_item::RenamePending {
        rx,
        collection_scope: if tree {
            crate::collection_store::CollectionSourceMigrationScope::Tree
        } else {
            crate::collection_store::CollectionSourceMigrationScope::Exact
        },
    });
    tx.send(Ok(crate::shell_file_ops::ShellRenameOutcome {
        target: old.to_owned(),
        new_path: new.to_owned(),
        operation: crate::shell_file_ops::ShellTransferOutcome::test_outcome(0, Ok(false), 0),
        converted_archive_rels: enumeration.converted_archive_rels,
        converted_archive_enumeration_incomplete: false,
    }))
    .unwrap();
    app.poll_rename_pending(&egui::Context::default());
    assert!(app.rename_pending.is_none());
}

fn finish_migrations(app: &mut App) {
    app.flush_rename_migration_journal().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while app.rename_migration_in_flight.is_some() || !app.rename_migration_queue.is_empty() {
        assert!(Instant::now() < deadline, "rename migration did not finish");
        app.poll_rename_migration_pending(&egui::Context::default());
        std::thread::yield_now();
    }
    assert!(app.rename_migration_boot_retry.is_empty());
}

fn consecutive_renames(pending_save: bool) {
    let mut env = setup_app();
    let data = env.tmp.path().to_path_buf();
    let app = &mut env.app;
    app.rename_migration_data_dir_override = Some(data.clone());
    app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
    let a = data.join("A");
    let b = data.join("B");
    let c = data.join("C");
    std::fs::create_dir_all(a.join("sub")).unwrap();
    std::fs::write(a.join("sub/f.rar"), b"archive").unwrap();
    let sources = [
        a.join("sub/f.rar"),
        b.join("sub/f.rar"),
        c.join("sub/f.rar"),
    ];
    let caches = sources.each_ref().map(|source| cache(&data, source));
    record_position(
        &data,
        &BookResumeKey::legacy(&caches[0]),
        &archive_pages(&caches[0], &[]),
        Some(&sources[0]),
        1,
    );
    let mut params = crate::adjustment::AdjustParams::default();
    params.brightness = 17.0;
    app.adjustment_db
        .as_ref()
        .unwrap()
        .set_page_params(&page_key(&caches[0]), &params)
        .unwrap();
    let pause = pending_save.then(|| app.book_resume_writer.as_ref().unwrap().pause_for_test());
    if pending_save {
        assert!(
            app.book_resume_writer
                .as_ref()
                .unwrap()
                .record_key_identity(
                    &BookResumeKey::legacy(&caches[0]),
                    2,
                    ReadingMeterValue::new(3, 4),
                    PageIdentity::for_item(&archive_pages(&caches[0], &[])[2], Some(&sources[0])),
                )
        );
    }
    std::fs::rename(&a, &b).unwrap();
    adopt_rename(app, &a, &b, true);
    std::fs::rename(&b, &c).unwrap();
    adopt_rename(app, &b, &c, true);
    assert!(app.rename_migration_in_flight.is_none());
    assert_eq!(app.rename_migration_queue.len(), 2);
    assert_eq!(
        app.rename_migration_queue[0].converted_archive_rels,
        vec![PathBuf::from("sub/f.rar")]
    );
    assert_eq!(
        app.rename_migration_queue[1].converted_archive_rels,
        vec![PathBuf::from("sub/f.rar")]
    );
    if let Some(pause) = pause {
        app.flush_rename_migration_journal().unwrap();
        app.try_start_next_rename_migration();
        assert!(
            app.rename_migration_in_flight.is_none(),
            "unfinished old-cache save blocks migration"
        );
        pause.send(()).unwrap();
    }
    finish_migrations(app);
    assert_restored(
        &data,
        &BookResumeKey::legacy(&caches[2]),
        &archive_pages(&caches[2], &[]),
        Some(&sources[2]),
        if pending_save { 2 } else { 1 },
    );
    for cache in &caches[..2] {
        assert!(read_position(&data, &BookResumeKey::legacy(cache)).is_none());
        assert!(
            app.adjustment_db
                .as_ref()
                .unwrap()
                .get_page_params(&page_key(cache))
                .is_none()
        );
    }
    assert_eq!(
        app.adjustment_db
            .as_ref()
            .unwrap()
            .get_page_params(&page_key(&caches[2]))
            .unwrap()
            .brightness,
        17.0
    );
}

#[test]
fn section1886_handler_consecutive_folder_renames_use_frozen_fs_inventory() {
    consecutive_renames(false);
}

#[test]
fn section1886_handler_consecutive_renames_wait_for_unfinished_cache_save() {
    consecutive_renames(true);
}

#[test]
fn section1886_handler_delete_waits_for_rename_and_suppresses_cache_stage() {
    let mut env = setup_app();
    let data = env.tmp.path().to_path_buf();
    let app = &mut env.app;
    app.rename_migration_data_dir_override = Some(data.clone());
    app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
    let a = data.join("A");
    let b = data.join("B");
    std::fs::create_dir(&a).unwrap();
    std::fs::write(a.join("f.rar"), b"archive").unwrap();
    let old_cache = cache(&data, &a.join("f.rar"));
    let new_cache = cache(&data, &b.join("f.rar"));
    record_position(
        &data,
        &BookResumeKey::legacy(&old_cache),
        &archive_pages(&old_cache, &[]),
        None,
        2,
    );
    let (_tx, rx) = mpsc::channel();
    app.rename_pending = Some(crate::ui_dialogs::rename_item::RenamePending {
        rx,
        collection_scope: crate::collection_store::CollectionSourceMigrationScope::Tree,
    });
    app.start_delete_files(
        &egui::Context::default(),
        vec![
            crate::ui_dialogs::context_menu::DeleteTarget::from_grid_item(&GridItem::Folder(
                a.clone(),
            ))
            .unwrap(),
        ],
    );
    assert!(app.delete_pending.is_none());
    std::fs::rename(&a, &b).unwrap();
    adopt_rename(app, &a, &b, true);
    // A completed disposable deletion, adopted by the actual deletion handler.
    std::fs::remove_file(b.join("f.rar")).unwrap();
    std::fs::remove_dir(&b).unwrap();
    let (tx, rx) = mpsc::channel();
    app.delete_pending = Some(crate::delete_worker::DeletePending {
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
        total: 1,
        succeeded: Vec::new(),
        failed: Vec::new(),
        purged_pdf_password_paths: Vec::new(),
        purge_deferred: false,
        source_scopes: vec![crate::delete_worker::DeleteSourceScope::Tree(b.clone())],
    });
    tx.send(crate::delete_worker::DeleteMsg::Batch {
        succeeded: vec![b.clone()],
        failed: Vec::new(),
        purged_pdf_password_paths: Vec::new(),
        purge_deferred: false,
    })
    .unwrap();
    tx.send(crate::delete_worker::DeleteMsg::Done {
        canceled: false,
        store_mutations: Default::default(),
    })
    .unwrap();
    app.poll_delete_pending();
    assert!(!app.rename_migration_queue[0].generic_pending);
    assert_eq!(app.rename_migration_queue[0].mappings.len(), 1);
    assert_eq!(app.rename_migration_queue[0].mappings[0].new_path, b);
    finish_migrations(app);
    assert!(read_position(&data, &BookResumeKey::legacy(&old_cache)).is_some());
    assert!(read_position(&data, &BookResumeKey::legacy(&new_cache)).is_none());
}

fn deliver_completion(app: &mut App, old: &Path, new: &Path, pairs: Vec<(PathBuf, PathBuf)>) {
    let (tx, rx) = mpsc::channel();
    app.rename_migration_in_flight = Some(RenameMigrationInFlight::Generic {
        job: migration::PathMigrationJob::shell_rename(old.to_owned(), new.to_owned(), false),
        rx,
    });
    tx.send(migration::RenameMigrationReport {
        rows: 1,
        errors: Vec::new(),
        store_mutations: Default::default(),
        panicked: false,
        converted_cache_pairs: pairs,
    })
    .unwrap();
    app.poll_rename_migration_pending(&egui::Context::default());
}

fn finish_fresh_page_edit_prepare(app: &mut App, ctx: &egui::Context) {
    let Some(edits::ReconcileStatus::Building(build)) = app.page_edit_reconcile_pending.as_ref()
    else {
        panic!("rename completion must leave an initial edit prepare");
    };
    assert_eq!(build.context_id, app.virtual_list_context_id());
    assert_eq!(build.items_generation, app.items_generation);
    assert_eq!(
        build.page_edit_revision, app.page_edit_revision,
        "completion notifications must precede preparation of the final metadata revision"
    );
    assert_eq!(build.item_count, app.items.len());

    // This is the ROOT update entry that registers writer wakes and polls the
    // mounted context. Keep its real bounded builder, DB worker and adoption.
    app.service_update_registered_writers(ctx);
    assert!(
        matches!(
            app.page_edit_reconcile_pending,
            Some(edits::ReconcileStatus::Running(_))
        ),
        "the four-page builder must start its real prepare worker"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.page_edit_snapshot.is_none() {
        assert!(
            Instant::now() < deadline,
            "fresh prepare did not arrive (pending={}, revision={})",
            match app.page_edit_reconcile_pending.as_ref() {
                Some(edits::ReconcileStatus::Building(_)) => "Building",
                Some(edits::ReconcileStatus::Running(_)) => "Running",
                Some(edits::ReconcileStatus::Failed { .. }) => "Failed",
                None => "None",
            },
            app.page_edit_revision
        );
        app.service_update_registered_writers(ctx);
        std::thread::yield_now();
    }
    assert!(app.page_edit_reconcile_pending.is_none());
    let snapshot = app.page_edit_snapshot.as_ref().unwrap();
    assert!(
        crate::page_edit_write_epoch::PAGE_EDIT_WRITES.accepts(snapshot.stamp.unwrap()),
        "the adopted DB read must belong to the current write epoch"
    );
}

#[test]
fn section1886_handler_success_and_completion_rewrite_presence_and_prepare_cache_async() {
    let mut env = setup_app();
    let data = env.tmp.path().to_path_buf();
    let app = &mut env.app;
    app.rename_migration_data_dir_override = Some(data.clone());
    app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
    let old = data.join("before.rar");
    let new = data.join("after.rar");
    let old_cache = cache(&data, &old);
    let new_cache = cache(&data, &new);
    let old_key = page_key(&old_cache);
    let new_key = page_key(&new_cache);
    app.adjusted_page_keys.insert(old_key.clone());
    adopt_rename(app, &old, &new, false);
    assert!(!app.adjusted_page_keys.contains(&old_key));
    assert!(app.adjusted_page_keys.contains(&new_key));
    app.rename_migration_queue.clear();
    // Model boot recovery: startup has loaded the old presence keys again.
    app.adjusted_page_keys.remove(&new_key);
    app.adjusted_page_keys.insert(old_key.clone());
    app.current_folder = Some(new_cache.clone());
    app.items = archive_pages(&new_cache, &[]);
    app.thumbnails = (0..app.items.len())
        .map(|_| ThumbnailState::Pending)
        .collect();
    app.page_edit_snapshot = None;
    app.rating_session_writes.insert(
        old_key.clone(),
        crate::app::RatingSessionWrite {
            generation: 1,
            stars: 4,
            explicit_user: true,
        },
    );
    app.thumb_edit_preview_keys.insert(2, new_key.clone());
    let sync_before = SYNC_REHYDRATIONS.with(|count| count.get());
    deliver_completion(app, &old, &new, vec![(old_cache, new_cache)]);
    assert!(!app.adjusted_page_keys.contains(&old_key));
    assert!(app.adjusted_page_keys.contains(&new_key));
    assert!(app.rating_session_writes.contains_key(&new_key));
    assert!(!app.rating_session_writes.contains_key(&old_key));
    assert!(matches!(
        app.page_edit_reconcile_pending,
        Some(edits::ReconcileStatus::Building(_))
    ));
    assert_eq!(SYNC_REHYDRATIONS.with(|count| count.get()), sync_before);
    assert!(matches!(app.thumbnails[2], ThumbnailState::Evicted));
}

#[test]
fn section1886_handler_rejects_prepare_read_before_migration_at_late_adoption() {
    let mut env = setup_app();
    let data = env.tmp.path().to_path_buf();
    let app = &mut env.app;
    let ctx = egui::Context::default();
    let old = data.join("old.rar");
    let new = data.join("new.rar");
    let old_cache = cache(&data, &old);
    let new_cache = cache(&data, &new);
    let mut params = crate::adjustment::AdjustParams::default();
    params.brightness = 29.0;
    app.adjustment_db
        .as_ref()
        .unwrap()
        .set_page_params(&page_key(&old_cache), &params)
        .unwrap();
    app.current_folder = Some(new_cache.clone());
    app.items = archive_pages(&new_cache, &[]);
    app.thumbnails = (0..app.items.len())
        .map(|_| ThumbnailState::Pending)
        .collect();
    app.page_edit_snapshot = None;
    let stale = edits::PageEditSnapshot::load_and_project_stable(
        &app.items,
        edits::PageEditAvailability::for_app(app),
        &AtomicBool::new(false),
    )
    .unwrap()
    .unwrap();
    assert!(stale.snapshot.adjustment.is_empty());
    // Leave the completed prepare queued, then commit migration, then adopt.
    let (tx, rx) = mpsc::channel();
    let stamp = stale.stamp;
    app.page_edit_reconcile_pending =
        Some(edits::ReconcileStatus::Running(edits::ReconcilePending {
            context_id: app.virtual_list_context_id(),
            items_generation: app.items_generation,
            page_edit_revision: app.page_edit_revision,
            order: None,
            cancel: Arc::new(AtomicBool::new(false)),
            receiver: rx,
            started_stamp: stamp,
            retry_attempt: 0,
        }));
    tx.send(Ok(Some(edits::ReconcileReady::Full(stale))))
        .unwrap();
    let report = migration::run_at(&data, &old, &new);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(!crate::page_edit_write_epoch::PAGE_EDIT_WRITES.accepts(stamp));
    app.service_update_registered_writers(&ctx);
    assert!(
        app.page_edit_snapshot.is_none(),
        "stale DB result must not be installed"
    );
    assert!(matches!(
        app.page_edit_reconcile_pending,
        Some(edits::ReconcileStatus::Building(_))
    ));
    app.rename_migration_data_dir_override = Some(data.clone());
    app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
    deliver_completion(app, &old, &new, report.converted_cache_pairs);
    finish_fresh_page_edit_prepare(app, &ctx);
    assert_eq!(
        app.page_edit_snapshot.as_ref().unwrap().adjustment[&page_key(&new_cache)].brightness,
        29.0
    );
    assert_eq!(app.adjustment_page_params[&2].brightness, 29.0);
}

#[test]
fn section1886_handler_original_path_prepare_uses_final_metadata_revision() {
    let mut env = setup_app();
    let data = env.tmp.path().to_path_buf();
    let app = &mut env.app;
    let ctx = egui::Context::default();
    app.rename_migration_data_dir_override = Some(data.clone());
    app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
    let old = data.join("old.zip");
    let new = data.join("new.zip");
    let mut params = crate::adjustment::AdjustParams::default();
    params.brightness = 37.0;
    app.adjustment_db
        .as_ref()
        .unwrap()
        .set_page_params(&page_key(&old), &params)
        .unwrap();
    app.current_folder = Some(new.clone());
    app.items = archive_pages(&new, &[]);
    app.thumbnails = (0..app.items.len())
        .map(|_| ThumbnailState::Pending)
        .collect();
    // A direct ZIP uses the original pair's existing async rehydrate branch
    // when it already owns a prepared snapshot from before migration.
    let stale = edits::PageEditSnapshot::load_and_project_stable(
        &app.items,
        edits::PageEditAvailability::for_app(app),
        &AtomicBool::new(false),
    )
    .unwrap()
    .unwrap();
    assert!(stale.snapshot.adjustment.is_empty());
    app.install_prepared_page_edits(stale);
    let report = migration::run_at(&data, &old, &new);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(report.converted_cache_pairs.is_empty());
    deliver_completion(app, &old, &new, report.converted_cache_pairs);
    finish_fresh_page_edit_prepare(app, &ctx);
    assert_eq!(
        app.page_edit_snapshot.as_ref().unwrap().adjustment[&page_key(&new)].brightness,
        37.0
    );
    assert_eq!(app.adjustment_page_params[&2].brightness, 37.0);
}

#[test]
fn section1886_many_candidates_visit_each_presence_rating_and_preview_key_once() {
    let mut env = setup_app();
    let app = &mut env.app;
    for set in [
        &mut app.adjusted_page_keys,
        &mut app.local_adjust_page_keys,
        &mut app.mask_page_keys,
        &mut app.conceal_page_keys,
        &mut app.comic_page_keys,
        &mut app.export_crop_page_keys,
    ] {
        set.clear();
    }
    app.rotation_page_keys.clear();
    app.rating_session_writes.clear();
    app.thumb_edit_preview_keys.clear();
    let pairs: Vec<_> = (0..400)
        .map(|i| {
            (
                PathBuf::from(format!("C:/cache/old{i}.zip")),
                PathBuf::from(format!("C:/cache/new{i}.zip")),
            )
        })
        .collect();
    for i in 0..1400 {
        let key = if i < 400 {
            page_key(&pairs[i].0)
        } else {
            format!("c:/unrelated/{i}.zip::page.jpg")
        };
        app.adjusted_page_keys.insert(key.clone());
        app.rating_session_writes.insert(
            key.clone(),
            crate::app::RatingSessionWrite {
                generation: 1,
                stars: 3,
                explicit_user: true,
            },
        );
        app.thumb_edit_preview_keys.insert(i, key);
    }
    // No current items: the production completion still visits the preview cache once.
    app.items.clear();
    app.current_folder = None;
    app.rename_migration_data_dir_override = Some(env.tmp.path().to_owned());
    app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
    deliver_completion(
        app,
        Path::new("C:/books/old"),
        Path::new("C:/books/new"),
        pairs.clone(),
    );
    assert_eq!(
        LAST_BATCH_VISITS.with(|stats| stats.get()),
        (1400, 1400, 1400)
    );
    for (_, new) in &pairs {
        assert!(app.adjusted_page_keys.contains(&page_key(new)));
        assert!(app.rating_session_writes.contains_key(&page_key(new)));
    }
}

#[test]
fn section1886_handler_migration_finishes_during_enumeration_before_first_edit_read() {
    let mut env = setup_app();
    let data = env.tmp.path().to_owned();
    let app = &mut env.app;
    app.rename_migration_data_dir_override = Some(data.clone());
    app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
    let old = data.join("before.rar");
    let new = data.join("after.rar");
    let old_cache = cache(&data, &old);
    let new_cache = cache(&data, &new);
    let mut params = crate::adjustment::AdjustParams::default();
    params.brightness = 31.0;
    app.adjustment_db
        .as_ref()
        .unwrap()
        .set_page_params(&page_key(&old_cache), &params)
        .unwrap();
    app.current_folder = Some(new_cache.clone());
    app.archive_source_override = Some(new.clone());
    app.items.clear();
    app.page_edit_snapshot = None;
    let (tx, rx) = mpsc::channel();
    app.zip_enumerate_pending = Some(crate::app::ZipEnumeratePending {
        zip_path: new_cache.clone(),
        input_seq: 1,
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
        presentation: crate::app::ZipEnumerationPresentation::Existing(
            crate::app::StartupListIntent::ExplicitList,
        ),
    });
    let report = migration::run_at(&data, &old, &new);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let sync_before = SYNC_REHYDRATIONS.with(|count| count.get());
    deliver_completion(app, &old, &new, report.converted_cache_pairs);
    assert!(app.zip_enumerate_pending.is_some());
    assert!(
        app.page_edit_reconcile_pending.is_none(),
        "enumeration owns first hydration"
    );
    assert_eq!(SYNC_REHYDRATIONS.with(|count| count.get()), sync_before);
    tx.send(Ok(crate::zip_loader::PreparedZipEnumeration::prepare(
        new_cache.clone(),
        crate::zip_loader::ZipEnumeration {
            entries: (0..4)
                .map(|page| crate::zip_loader::ZipImageEntry {
                    entry_name: format!("{page:02}.jpg"),
                    mtime: 1,
                    uncompressed_size: 10,
                    container_ancestry: Vec::new(),
                })
                .collect(),
            has_foreign_archives: false,
            legacy_renames: Vec::new(),
        },
        app.settings.file_type_visibility.clone(),
    )))
    .unwrap();
    app.poll_zip_enumerate();
    app.finish_page_identity_preparation_for_test();
    let index = app
        .items
        .iter()
        .position(|item| {
            crate::edit_source::page_key_for_grid_item(item).as_deref()
                == Some(page_key(&new_cache).as_str())
        })
        .unwrap();
    assert_eq!(app.adjustment_page_params[&index].brightness, 31.0);
}

#[test]
#[cfg(windows)]
fn section1886_handler_rehydrates_only_matching_parked_cache_context() {
    let mut env = setup_app();
    let data = env.tmp.path().to_owned();
    let app = &mut env.app;
    app.rename_migration_data_dir_override = Some(data.clone());
    app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
    let old = data.join("old.rar");
    let new = data.join("new.rar");
    let old_cache = cache(&data, &old);
    let new_cache = cache(&data, &new);
    app.current_folder = Some(data.join("unrelated"));
    app.mask_pages.insert(73);
    let unrelated_generation = app.items_generation;
    let context = app.push_window_context_for_test(&egui::Context::default(), 1886001, |owner| {
        owner.current_folder = Some(new_cache.clone());
        owner.items = archive_pages(&new_cache, &[]);
        owner.page_edit_snapshot = None;
    });
    let unrelated = app.push_window_context_for_test(&egui::Context::default(), 1886002, |owner| {
        owner.current_folder = Some(data.join("other-context"));
        owner.items = Vec::new();
        owner.mask_pages.insert(91);
    });
    deliver_completion(app, &old, &new, vec![(old_cache, new_cache)]);
    assert!(app.mask_pages.contains(&73));
    assert_eq!(app.items_generation, unrelated_generation);
    app.with_viewer_context(context, |owner| {
        assert!(matches!(
            owner.page_edit_reconcile_pending,
            Some(edits::ReconcileStatus::Building(_))
        ));
    })
    .unwrap();
    app.with_viewer_context(unrelated, |owner| {
        assert!(owner.mask_pages.contains(&91));
        assert!(owner.page_edit_reconcile_pending.is_none());
    })
    .unwrap();
}
