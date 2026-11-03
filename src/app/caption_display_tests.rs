use super::*;
use crate::rename_key_migration::StoreMutationEffects;

fn caption_mutation() -> StoreMutationEffects {
    let descriptor = crate::rename_key_migration::STORES
        .iter()
        .find(|descriptor| descriptor.file == "ai_caption.db")
        .unwrap();
    let mut effects = StoreMutationEffects::default();
    effects.record_rows_changed(descriptor, 1);
    effects
}

#[derive(Clone, Copy)]
struct DisplayEpoch {
    display: u64,
    caption: u64,
}

fn seed_display(app: &mut AppTestEnvForTest) -> DisplayEpoch {
    let index_data = app.tmp.path().join("caption-index");
    let caption = app
        .caption_epoch_for_test(&index_data)
        .load(Ordering::Acquire);
    app.caption_display.seed_for_test("c:/images/changed.jpg");
    app.caption_display
        .seed_for_test("d:/images/other-viewer.jpg");
    DisplayEpoch {
        display: app.caption_display.epoch_for_test(),
        caption,
    }
}

fn assert_caption_epoch(app: &App, expected: u64) {
    assert_eq!(
        app.indexer_init
            .as_ref()
            .unwrap()
            .caption_epoch()
            .load(Ordering::Acquire),
        expected
    );
}

fn assert_display_cleared(app: &App, old_epoch: DisplayEpoch) {
    assert!(
        !app.caption_display
            .contains_for_test("c:/images/changed.jpg")
    );
    assert!(
        !app.caption_display
            .contains_for_test("d:/images/other-viewer.jpg")
    );
    assert_ne!(app.caption_display.epoch_for_test(), old_epoch.display);
}

#[test]
fn caption_store_mutation_clears_shared_display_with_identity_setting_disabled() {
    let mut app = setup_app_for_test();
    app.settings.edit_restore_prompt_enabled = false;
    // Match the actual settings transition: setup may already own a startup loader.
    app.sync_content_identity_detection_setting(true);
    assert!(app.content_identity_index_load_pending.is_none());
    let epoch = seed_display(&mut app);
    app.apply_content_identity_store_mutations(StoreMutationEffects::default());
    assert_eq!(app.caption_display.epoch_for_test(), epoch.display);
    assert_caption_epoch(&app, epoch.caption);
    assert!(
        app.caption_display
            .contains_for_test("c:/images/changed.jpg")
    );

    app.apply_content_identity_store_mutations(caption_mutation());
    assert_display_cleared(&app, epoch);
    assert_caption_epoch(&app, epoch.caption + 1);
    assert!(app.content_identity_index_load_pending.is_none());
}

#[test]
fn caption_rename_completion_clears_shared_display() {
    let mut app = setup_app_for_test();
    app.settings.edit_restore_prompt_enabled = false;
    app.rename_migration_data_dir_override = Some(app.tmp.path().to_path_buf());
    app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
    let (tx, rx) = std::sync::mpsc::channel();
    app.rename_migration_in_flight = Some(RenameMigrationInFlight::Generic {
        job: crate::rename_key_migration::PathMigrationJob::shell_rename(
            app.tmp.path().join("old.jpg"),
            app.tmp.path().join("new.jpg"),
            false,
        ),
        rx,
    });
    tx.send(crate::rename_key_migration::RenameMigrationReport {
        rows: 1,
        errors: Vec::new(),
        store_mutations: caption_mutation(),
        panicked: false,
    })
    .unwrap();
    let epoch = seed_display(&mut app);

    app.poll_rename_migration_pending(&egui::Context::default());

    assert_display_cleared(&app, epoch);
    assert_caption_epoch(&app, epoch.caption + 1);
}

#[test]
fn caption_delete_done_completion_clears_shared_display() {
    let mut app = setup_app_for_test();
    app.settings.edit_restore_prompt_enabled = false;
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(crate::delete_worker::DeleteMsg::Done {
        canceled: false,
        store_mutations: caption_mutation(),
    })
    .unwrap();
    app.delete_pending = Some(crate::delete_worker::DeletePending {
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        rx,
        total: 0,
        succeeded: Vec::new(),
        failed: Vec::new(),
        purged_pdf_password_paths: Vec::new(),
        purge_deferred: false,
        source_scopes: Vec::new(),
    });
    let epoch = seed_display(&mut app);

    app.poll_delete_pending();

    assert!(app.delete_pending.is_none());
    assert_display_cleared(&app, epoch);
    assert_caption_epoch(&app, epoch.caption + 1);
}

#[test]
fn caption_delete_purge_retry_completion_clears_shared_display() {
    let mut app = setup_app_for_test();
    app.settings.edit_restore_prompt_enabled = false;
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(crate::metadata_cleanup::DeletePurgeRetryReport {
        attempted: 1,
        purged: 1,
        rows: 1,
        store_mutations: caption_mutation(),
        ..Default::default()
    })
    .unwrap();
    app.delete_purge_retry_pending = Some(crate::metadata_cleanup::DeletePurgeRetryPending { rx });
    let epoch = seed_display(&mut app);

    app.poll_delete_purge_retry(&egui::Context::default());

    assert!(app.delete_purge_retry_pending.is_none());
    assert_display_cleared(&app, epoch);
    assert_caption_epoch(&app, epoch.caption + 1);
}

#[test]
fn caption_cleanup_completion_clears_shared_display_before_empty_key_return() {
    let mut app = setup_app_for_test();
    app.settings.edit_restore_prompt_enabled = false;
    let epoch = seed_display(&mut app);

    app.apply_metadata_cleanup_result(&crate::metadata_cleanup::DeleteReport {
        store_mutations: caption_mutation(),
        ..Default::default()
    });

    assert_display_cleared(&app, epoch);
    assert_caption_epoch(&app, epoch.caption + 1);
}

#[test]
fn caption_book_store_completion_clears_shared_display() {
    let mut app = setup_app_for_test();
    app.settings.edit_restore_prompt_enabled = false;
    let epoch = seed_display(&mut app);

    app.adopt_book_store_summary(crate::books::BookStoreSummary {
        store_mutations: caption_mutation(),
        ..Default::default()
    });

    assert_display_cleared(&app, epoch);
    assert_caption_epoch(&app, epoch.caption + 1);
}

#[test]
fn caption_book_append_undo_completion_clears_shared_display() {
    let mut app = setup_app_for_test();
    app.settings.edit_restore_prompt_enabled = false;
    let epoch = seed_display(&mut app);
    let folder = app.tmp.path().join("book");

    app.finish_book_append_undo(crate::book_append_undo::BookAppendUndoResult {
        book_name: "book".into(),
        folder,
        succeeded: Vec::new(),
        failed: Vec::new(),
        purge_errors: Vec::new(),
        store_mutations: caption_mutation(),
    });

    assert_display_cleared(&app, epoch);
    assert_caption_epoch(&app, epoch.caption + 1);
}

#[test]
fn caption_book_append_metadata_completion_only_clears_for_deleted_captions() {
    let mut app = setup_app_for_test();
    let mut summary = crate::books::BookAppendSummary {
        book_name: "book".into(),
        folder: app.tmp.path().join("book"),
        added: 1,
        first_path: None,
        edit_copies: Vec::new(),
        semantic_copies: Vec::new(),
        erase_fallback_pages: 0,
        skipped_hidden: 0,
        undo_files: None,
        metadata_copy: Default::default(),
    };
    let epoch = seed_display(&mut app);
    app.finish_book_append_metadata(&summary);
    assert_eq!(app.caption_display.epoch_for_test(), epoch.display);
    assert_caption_epoch(&app, epoch.caption);
    assert!(
        app.caption_display
            .contains_for_test("c:/images/changed.jpg")
    );

    summary.metadata_copy.captions_changed = true;
    app.finish_book_append_metadata(&summary);
    assert_display_cleared(&app, epoch);
    assert_caption_epoch(&app, epoch.caption + 1);
}

fn metadata_refresh(
    contexts: Vec<metadata_import_refresh::ContextResult>,
) -> metadata_import_refresh::RefreshResult {
    metadata_import_refresh::RefreshResult {
        contexts,
        page_snapshot: None,
        errors: Vec::new(),
    }
}

#[test]
fn caption_metadata_import_completion_only_clears_when_captions_changed() {
    let mut app = setup_app_for_test();
    let epoch = seed_display(&mut app);
    let (_, stale) = app
        .apply_metadata_import_terminal_refresh(metadata_refresh(Vec::new()), Default::default());
    assert!(!stale);
    assert_eq!(app.caption_display.epoch_for_test(), epoch.display);
    assert_caption_epoch(&app, epoch.caption);
    assert!(
        app.caption_display
            .contains_for_test("c:/images/changed.jpg")
    );

    let (_, stale) = app.apply_metadata_import_terminal_refresh(
        metadata_refresh(Vec::new()),
        crate::metadata_transfer::ImportChangedSections {
            ai_captions: true,
            ..Default::default()
        },
    );
    assert!(!stale);
    assert_display_cleared(&app, epoch);
    assert_caption_epoch(&app, epoch.caption + 1);
}

#[test]
fn caption_metadata_import_stale_refresh_still_clears_committed_captions() {
    let mut app = setup_app_for_test();
    let context = metadata_import_refresh::ContextResult {
        context_id: app.projected_viewer_context_id(),
        items_generation: app.items_generation.wrapping_add(1),
        rating_cache: None,
        tags_cache: None,
        current_rating: None,
        page_state: None,
        folder_pin_map: None,
        folder_pin_materializations: None,
        video_pin_blobs: None,
        video_items: None,
        container_state: None,
        shift_read: None,
    };
    let epoch = seed_display(&mut app);

    let (_, stale) = app.apply_metadata_import_terminal_refresh(
        metadata_refresh(vec![context]),
        crate::metadata_transfer::ImportChangedSections {
            ai_captions: true,
            ..Default::default()
        },
    );

    assert!(stale);
    assert_display_cleared(&app, epoch);
    assert_caption_epoch(&app, epoch.caption + 1);
}

#[test]
fn caption_thumbnail_pin_import_and_store_effects_invalidate_all_viewer_values() {
    let mut app = setup_app_for_test();
    let epoch = seed_display(&mut app);
    app.apply_metadata_import_terminal_refresh(
        metadata_refresh(Vec::new()),
        crate::metadata_transfer::ImportChangedSections {
            thumbnail_pins: true,
            ..Default::default()
        },
    );
    assert_display_cleared(&app, epoch);
    assert_caption_epoch(&app, epoch.caption);
    for file in ["folder_thumb_pins.db", "video_pins.db"] {
        let descriptor = crate::rename_key_migration::STORES
            .iter()
            .find(|store| store.file == file)
            .unwrap();
        let mut effects = StoreMutationEffects::default();
        effects.record_rows_changed(descriptor, 1);
        let epoch = seed_display(&mut app);
        app.apply_content_identity_store_mutations(effects);
        assert_display_cleared(&app, epoch);
        assert_caption_epoch(&app, epoch.caption);
    }
}

#[test]
fn caption_folder_pin_set_and_remove_clear_at_database_mutation_boundary() {
    use crate::folder_thumb_pins::{FileKind, FolderPinSource};
    let mut app = setup_app_for_test();
    let folder = app.tmp.path().join("book");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("cover.png"), b"cover").unwrap();
    let epoch = seed_display(&mut app);
    assert!(app.set_folder_thumb_pin(
        &folder,
        FolderPinSource::File {
            rel: "cover.png".into(),
            kind: FileKind::Image
        }
    ));
    assert_display_cleared(&app, epoch);
    assert_caption_epoch(&app, epoch.caption);
    // The ZIP caller can consume the thumbnail dirty marker immediately;
    // invalidation must already have happened inside the writer itself.
    app.folder_thumb_pin_dirty.clear();
    let epoch = seed_display(&mut app);
    assert!(app.remove_folder_thumb_pin(&folder));
    assert_display_cleared(&app, epoch);
    assert_caption_epoch(&app, epoch.caption);
}

#[test]
fn caption_book_panel_key_matches_saved_key_and_physical_stars_for_converted_archive() {
    use crate::app::book_cover::BookKind;
    let mut app = setup_app_for_test();
    app.fullscreen_idx = Some(0);
    let folder = app.tmp.path().join("folder-book");
    app.current_folder = Some(folder.clone());
    let (key, path, kind) = app.current_caption_book_target().unwrap();
    assert_eq!(
        key,
        crate::ai_caption_db::CaptionDb::normalize_path(&folder)
    );
    assert_eq!(key, app.physical_container_rating_key().unwrap().key);
    assert_eq!(path, folder);
    assert_eq!(kind, BookKind::Folder);
    app.current_folder = Some(app.tmp.path().join("folder.pdf"));
    assert_eq!(
        app.current_caption_book_target().unwrap().2,
        BookKind::Folder
    );

    let original = app.tmp.path().join("original.7z");
    let converted = app.tmp.path().join("cached.zip");
    app.current_folder = Some(converted.clone());
    app.archive_source_override = Some(original.clone());
    app.zip_nav = Some(crate::zip_tree::ZipNavState::new(std::sync::Arc::new(
        crate::zip_tree::ZipTree::build(
            converted,
            vec![crate::zip_loader::ZipImageEntry {
                entry_name: "cover.png".into(),
                uncompressed_size: 1,
                mtime: 0,
                container_ancestry: Vec::new(),
            }],
        ),
    )));
    let (key, path, kind) = app.current_caption_book_target().unwrap();
    assert_eq!(path, original);
    assert_eq!(key, crate::ai_caption_db::CaptionDb::normalize_path(&path));
    assert_eq!(key, app.physical_container_rating_key().unwrap().key);
    assert_eq!(
        kind,
        BookKind::Archive(crate::archive_converter::ArchiveFormat::SevenZ)
    );
    app.fullscreen_idx = None;
    assert!(app.current_caption_book_target().is_none());
}

#[test]
fn caption_changes_before_ready_are_resampled_on_startup_adoption() {
    let mut app = setup_app_for_test();
    let data = app.tmp.path().join("caption-index-adoption");
    let manager = crate::indexer_manager::IndexerManager::new_at(
        &data,
        &[],
        crate::settings::IndexerSpeedProfile::default(),
        Arc::clone(&app.activity_gate),
        vec![],
    )
    .unwrap();
    let epoch = manager.caption_epoch();
    app.indexer_init = IndexerInit::NotStarted;
    // A background manager may already have drained startup pending rows while
    // App still cannot deliver a committed job/import terminal to it.
    app.notify_caption_changed();
    assert_eq!(epoch.load(Ordering::Acquire), 0);
    app.adopt_startup_init(
        crate::indexer_manager::StartupInitOutcome::Ready(manager),
        false,
    );
    assert_eq!(epoch.load(Ordering::Acquire), 1);
    app.notify_caption_changed();
    assert_eq!(epoch.load(Ordering::Acquire), 2);
}
