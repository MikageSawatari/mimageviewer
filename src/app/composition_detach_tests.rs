//! Ordinary-folder persistence must keep the released sidecar/rename boundary.
//! These tests run real generic migrations but retain their App ownership;
//! final persistence must flush dirty tags without harvesting those migrations.

use super::tests::phase_c_support::{AppTestEnv, setup_app};
use super::{PersistScope, RenameMigrationInFlight, RenameMigrationJournalAdmission};
use crate::rename_key_migration::{PathMigrationJob, RenameMigrationReport};
use std::{path::PathBuf, time::Duration};

struct PlainFolderFixture {
    app: AppTestEnv,
    roots: [PathBuf; 3],
    in_flight: PathMigrationJob,
    queued: PathMigrationJob,
    // Receive the production report, but do not let App adopt it. This makes
    // the in-flight ownership deterministic without sleep or fake completions.
    _unadopted_report: RenameMigrationReport,
}

fn plain_folder_fixture() -> PlainFolderFixture {
    let mut app = setup_app();
    app.book_resume_writer = None;
    app.settings.sidecar_backup_enabled = true;
    app.settings.tag_sidecar_backup_enabled = true;
    app.rename_migration_data_dir_override = Some(app.tmp.path().into());
    app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
    let roots = ["in-flight-folder", "queued-folder", "unrelated-folder"]
        .map(|name| app.tmp.path().join(name));
    for root in &roots {
        std::fs::create_dir(root).unwrap();
        std::fs::write(root.join("a.png"), b"ordinary folder page").unwrap();
        let mut sidecar = crate::sidecar::SidecarFile::new(root.clone());
        sidecar.set_tags("a.png", ["dirty-before-exit"]);
        app.sidecars.insert(root.clone(), sidecar);
    }

    let old = roots[0].join("before.png");
    let new = roots[0].join("after.png");
    std::fs::write(&old, b"renamed page").unwrap();
    std::fs::rename(&old, &new).unwrap();
    app.spawn_rename_key_migration(
        old,
        new,
        crate::collection_store::CollectionSourceMigrationScope::Exact,
    );
    app.flush_rename_migration_journal().unwrap();
    app.try_start_next_rename_migration();
    let (in_flight, report) = match app.rename_migration_in_flight.as_ref().unwrap() {
        RenameMigrationInFlight::Generic { job, rx } => (
            job.clone(),
            rx.recv_timeout(Duration::from_secs(30)).unwrap(),
        ),
        _ => panic!("the production generic worker must own the first rename"),
    };
    assert!(!report.panicked);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(in_flight.generic_pending);

    let old = roots[1].join("before.png");
    let new = roots[1].join("after.png");
    std::fs::write(&old, b"queued renamed page").unwrap();
    std::fs::rename(&old, &new).unwrap();
    app.spawn_rename_key_migration(
        old,
        new,
        crate::collection_store::CollectionSourceMigrationScope::Exact,
    );
    assert_eq!(app.rename_migration_queue.len(), 1);
    let queued = app.rename_migration_queue[0].clone();
    assert!(queued.generic_pending);
    app.flush_rename_migration_journal().unwrap();
    for root in &roots {
        assert!(app.sidecars[root].is_dirty());
        assert!(!root.join(crate::sidecar::SIDECAR_FILENAME).exists());
    }
    PlainFolderFixture {
        app,
        roots,
        in_flight,
        queued,
        _unadopted_report: report,
    }
}

fn assert_migrations_unconsumed(fixture: &PlainFolderFixture) {
    match fixture.app.rename_migration_in_flight.as_ref().unwrap() {
        RenameMigrationInFlight::Generic { job, .. } => assert_eq!(job, &fixture.in_flight),
        _ => panic!("sidecar persistence must not consume the generic migration"),
    }
    assert_eq!(fixture.app.rename_migration_queue.len(), 1);
    assert_eq!(fixture.app.rename_migration_queue[0], fixture.queued);
    assert_eq!(
        crate::rename_key_migration::journal_load(fixture.app.tmp.path()).unwrap(),
        vec![fixture.in_flight.clone(), fixture.queued.clone()],
        "unfinished generic work remains in its existing durable journal"
    );
}

fn assert_plain_folder_tags_on_disk(fixture: &PlainFolderFixture) {
    for root in &fixture.roots {
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join(crate::sidecar::SIDECAR_FILENAME)).unwrap(),
        )
        .unwrap();
        assert_eq!(json["version"], 1);
        assert!(json.get("folder_composition").is_none());
        assert_eq!(
            json["items"]["a.png"]["tags"],
            serde_json::json!(["#dirty-before-exit"])
        );
        assert!(!fixture.app.sidecars[root].is_dirty());
    }
}

#[test]
fn plain_folder_final_exit_flushes_dirty_tags_with_generic_rename_owners_pending() {
    let mut fixture = plain_folder_fixture();
    let ctx = egui::Context::default();
    let mut close = egui::RawInput::default();
    close
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    let output = ctx.run(close, |ctx| {
        fixture.app.defer_composition_editor_root_close(ctx)
    });
    assert!(
        !output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::CancelClose)),
        "ordinary folders do not acquire a composition exit barrier"
    );

    fixture
        .app
        .persist_window_state_and_flush(PersistScope::ProcessIsStopping);

    // This boundary already waits for the sidecar writer. Read actual disk
    // bytes, rather than accepting the writer's read-through pending snapshot.
    assert_plain_folder_tags_on_disk(&fixture);
    assert_migrations_unconsumed(&fixture);
}

#[test]
fn plain_folder_tray_flush_queues_dirty_tags_with_generic_rename_owners_pending() {
    let mut fixture = plain_folder_fixture();

    fixture
        .app
        .persist_window_state_and_flush(PersistScope::ProcessKeepsRunning);

    // Tray persistence only queues writes. A fresh reader must immediately
    // see them through the existing pending-writer read path.
    for root in &fixture.roots {
        let loaded = crate::sidecar::SidecarFile::load(root);
        assert_eq!(
            loaded.items()["a.png"].tags.as_ref().unwrap(),
            &["#dirty-before-exit"]
        );
        assert!(!fixture.app.sidecars[root].is_dirty());
    }
    assert_migrations_unconsumed(&fixture);
    crate::sidecar::wait_for_pending_writes();
    assert_plain_folder_tags_on_disk(&fixture);
}
