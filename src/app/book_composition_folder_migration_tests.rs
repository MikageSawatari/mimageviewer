//! Real migration completions are held before App adoption; no timing polls.
use super::*;
use crate::app::tests::phase_c_support::setup_app;
use crate::app::{RenameMigrationInFlight, RenameMigrationJournalAdmission};
use crate::book_composition::{
    BookComposition, CompositionPageRef, CompositionUnit, ReadingDirection, SingleCompanion,
    SinglePlacement,
};
use std::{path::PathBuf, time::Duration};

const WAIT: Duration = Duration::from_secs(30);

fn record() -> BookComposition {
    BookComposition::new(
        ReadingDirection::Rtl,
        ["a.png", "b.png"]
            .map(|name| CompositionUnit::Single {
                page: CompositionPageRef::RelativePath { path: name.into() },
                placement: SinglePlacement::Center,
                companion: SingleCompanion::Vacant,
            })
            .to_vec(),
    )
}

#[test]
fn folder_page_rename_holds_dirty_sidecar_through_mirror_and_cache_adoption() {
    for (book_page_move, backup_enabled) in
        [(false, true), (false, false), (true, true), (true, false)]
    {
        let mut app = setup_app();
        app.book_resume_writer = None;
        app.settings.sidecar_backup_enabled = backup_enabled;
        app.settings.tag_sidecar_backup_enabled = true;
        app.rename_migration_data_dir_override = Some(app.tmp.path().into());
        app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
        let root = app.tmp.path().join("rename-folder");
        let neighbor = app.tmp.path().join("unrelated-folder");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&neighbor).unwrap();
        for name in ["a.png", "b.png"] {
            image::RgbaImage::new(8, 12).save(root.join(name)).unwrap();
        }
        let (service, process) =
            crate::book_composition::BookCompositionService::spawn_at(app.tmp.path().into(), None)
                .unwrap();
        app.install_book_composition_runtime(service.clone());
        let scope = BookScopeKey::folder(&root).unwrap();
        let original = record();
        service
            .save_folder(scope.clone(), original.clone(), true)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .unwrap();
        let original = service
            .prepare(scope.clone(), PageDomain::ImageFolder)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .unwrap()
            .unwrap()
            .composition;
        service.drain_changed_scope_keys();
        let mut cached = crate::sidecar::SidecarFile::load(&root);
        cached.set_tags("a.png", ["dirty-before-rename"]);
        cached.set_dirty_since_for_test(std::time::Instant::now() - Duration::from_secs(600));
        app.sidecars.insert(root.clone(), cached);
        let mut unrelated = crate::sidecar::SidecarFile::new(neighbor.clone());
        unrelated.set_tags("other.png", ["independent"]);
        unrelated.set_dirty_since_for_test(std::time::Instant::now() - Duration::from_secs(600));
        app.sidecars.insert(neighbor.clone(), unrelated);
        let old = root.join("b.png");
        let new = root.join("renamed.png");
        std::fs::rename(&old, &new).unwrap();
        if book_page_move {
            app.apply_book_page_edit_moves_with_journal(
                &[crate::books::BookPathMapping { from: old, to: new }],
                None,
                None,
            );
        } else {
            app.spawn_rename_key_migration(
                old,
                new,
                crate::collection_store::CollectionSourceMigrationScope::Exact,
            );
        }
        // Dispatch is gated by the existing journal writer's ACK.
        app.flush_rename_migration_journal().unwrap();
        if book_page_move {
            app.book_bookmark_service
                .as_ref()
                .unwrap()
                .fence_for_test()
                .recv_timeout(WAIT)
                .unwrap();
            app.poll_book_bookmarks();
        }
        app.try_start_next_rename_migration();
        // This is the production worker's report, after its mirror and the
        // maintenance lease's immutable-snapshot publication. Keep the owner
        // in App while holding back only result adoption.
        let report = match app.rename_migration_in_flight.as_ref().unwrap() {
            RenameMigrationInFlight::Generic {
                rx,
                folder_backup_enabled: captured,
                ..
            } => {
                assert_eq!(*captured, backup_enabled);
                rx.recv_timeout(WAIT).unwrap()
            }
            RenameMigrationInFlight::BookComposition { job, rx } => {
                assert_eq!(
                    job.stage,
                    crate::rename_key_migration::PathMigrationStage::BookComposition {
                        backup_enabled
                    }
                );
                rx.recv_timeout(WAIT).unwrap()
            }
            _ => panic!("page migration expected"),
        };
        assert!(!report.panicked);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report.store_mutations.book_composition_changed());
        let updated = service
            .prepare(scope.clone(), PageDomain::ImageFolder)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .unwrap()
            .unwrap()
            .composition;
        assert_ne!(updated, original);
        let expected = if backup_enabled { &updated } else { &original };
        assert_eq!(
            crate::sidecar::SidecarFile::load(&root).folder_composition(),
            Some(expected)
        );
        let before = std::fs::read(root.join(crate::sidecar::SIDECAR_FILENAME)).unwrap();
        assert_eq!(app.sidecars[&root].folder_composition(), Some(&original));
        let ctx = egui::Context::default();
        let mut close = egui::RawInput::default();
        close
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        let output = ctx.run(close, |ctx| app.defer_composition_editor_root_close(ctx));
        assert_eq!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose)),
            backup_enabled
        );
        app.flush_all_sidecars();
        app.flush_periodic_sidecars();
        app.flush_and_retire_unowned_sidecars();
        crate::sidecar::wait_for_pending_writes();
        if backup_enabled {
            assert_eq!(
                std::fs::read(root.join(crate::sidecar::SIDECAR_FILENAME)).unwrap(),
                before,
                "unadopted migration owns writes after its mirror"
            );
            assert!(app.sidecars[&root].is_dirty());
        }
        assert_eq!(
            crate::sidecar::SidecarFile::load(&neighbor).items()["other.png"]
                .tags
                .as_ref()
                .unwrap(),
            &["#independent"]
        );
        let (tx, rx) = mpsc::channel();
        tx.send(report).unwrap();
        match &mut app.rename_migration_in_flight {
            Some(RenameMigrationInFlight::Generic { rx: receiver, .. })
            | Some(RenameMigrationInFlight::BookComposition { rx: receiver, .. }) => *receiver = rx,
            _ => unreachable!(),
        }
        // The captured worker policy owns adoption, even when the setting is
        // changed before this result is consumed. No service-poll ordering is needed.
        app.settings.sidecar_backup_enabled = !backup_enabled;
        app.poll_rename_migration_pending(&egui::Context::default());
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            app.defer_composition_editor_root_close(ctx)
        });
        assert_eq!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Close)),
            backup_enabled
        );
        if backup_enabled {
            assert_eq!(app.sidecars[&root].folder_composition(), Some(expected));
        } else {
            // Backup-OFF claims no mirror; ordinary cache retirement remains valid.
            assert!(!app.sidecars.contains_key(&root));
        }
        app.flush_all_sidecars();
        crate::sidecar::wait_for_pending_writes();
        let saved = crate::sidecar::SidecarFile::load(&root);
        assert_eq!(saved.folder_composition(), Some(expected));
        assert_eq!(
            saved.items()["a.png"].tags.as_ref().unwrap(),
            &["#dirty-before-rename"]
        );
        process.shutdown_and_join();
    }
}

#[test]
fn folder_rename_write_claims_match_parents_and_tree_component_boundaries() {
    let mut app = setup_app();
    app.settings.sidecar_backup_enabled = true;
    let parent = PathBuf::from(r"C:\books\one");
    let child = parent.join("child");
    let neighbor = PathBuf::from(r"C:\books\one-more");
    for folder in [&parent, &child, &neighbor] {
        app.sidecars.insert(
            folder.clone(),
            crate::sidecar::SidecarFile::new(folder.clone()),
        );
    }
    app.rename_migration_queue.push_back(
        crate::rename_key_migration::PathMigrationJob::shell_rename(
            parent.join("a.png"),
            parent.join("b.png"),
            false,
        ),
    );
    assert_eq!(app.composition_sidecar_flush_roots(), vec![parent.clone()]);
    app.rename_migration_queue.clear();
    app.rename_migration_queue.push_back(
        crate::rename_key_migration::PathMigrationJob::shell_rename(
            parent.clone(),
            PathBuf::from(r"C:\books\new"),
            true,
        ),
    );
    let roots = app.composition_sidecar_flush_roots();
    assert!(roots.contains(&parent));
    assert!(roots.contains(&child));
    assert!(!roots.contains(&neighbor));
}
