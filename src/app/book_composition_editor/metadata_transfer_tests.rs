#![cfg(test)]

//! Sent editor saves must finish at their existing receipt owner before any
//! metadata maintenance producer is admitted, even after the viewer retires.
#![cfg(windows)]

use super::*;
use crate::app::book_composition_runtime::BookCompositionSource;
use crate::app::book_materialization::{self, BookScope, ProjectionRecipe};
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::grid_item::{GridItem, ThumbnailState};
use crate::ui_dialogs::metadata_transfer::save_ticket_test_support as transfer;
use std::{io::Write, path::Path, sync::Arc, time::Duration};

const WAIT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug)]
enum SourceKind {
    Zip,
    Pdf,
    Epub,
}

struct Fixture {
    app: AppTestEnv,
    service: BookCompositionService,
    process: Option<BookCompositionProcessOwner>,
    scope: BookScopeKey,
    domain: PageDomain,
    owner: ViewerContextId,
    _pdf_backend: Option<crate::pdf_loader::RemotePdfTestBackend>,
    _epub_pin: Option<crate::pdf_loader::TestEpubPin>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Transfer workers must release maintenance before the service exits.
        self.app.metadata_transfer = None;
        if let Some(process) = self.process.take() {
            process.shutdown_and_join();
        }
    }
}

fn write_pdf(path: &Path) {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R] /Count 4 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] >>",
    ];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Root 1 0 R /Size 7 >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    std::fs::write(path, bytes).unwrap();
}

fn fixture(kind: SourceKind) -> Fixture {
    let mut app = setup_app();
    app.book_resume_writer =
        crate::book_resume_db::BookResumeWriter::spawn_at(app.tmp.path().join("book_resume.db"));
    assert!(app.book_resume_writer.is_some());
    let destination = app.tmp.path().join("unrelated-transfer-location");
    std::fs::create_dir(&destination).unwrap();
    let main_image = destination.join("main.png");
    image::RgbaImage::new(8, 16).save(&main_image).unwrap();
    crate::metadata_transfer::export_at(
        app.tmp.path(),
        &destination,
        false,
        &std::sync::atomic::AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    app.current_folder = Some(destination);
    app.current_folder_last_mtime = Some(std::time::SystemTime::UNIX_EPOCH);
    app.items = vec![GridItem::Image(main_image)];
    app.image_metas = vec![None];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.visible_indices = vec![0];
    app.projection_recipe = ProjectionRecipe::OrdinaryList;
    let (service, process) = BookCompositionService::spawn_at(app.tmp.path().into(), None).unwrap();
    app.install_book_composition_runtime(service.clone());
    let mut backend = None;
    let mut pin = None;
    let (scope, domain, source, recipe, rows) = match kind {
        SourceKind::Zip => {
            let logical = app.tmp.path().join("saved-book.zip");
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&logical).unwrap());
            for name in ["a.png", "b.png", "c.png", "d.png"] {
                zip.start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                let mut png = std::io::Cursor::new(Vec::new());
                image::DynamicImage::ImageRgba8(image::RgbaImage::new(8, 16))
                    .write_to(&mut png, image::ImageFormat::Png)
                    .unwrap();
                zip.write_all(png.get_ref()).unwrap();
            }
            zip.finish().unwrap();
            let entries = crate::zip_loader::enumerate_image_entries_detailed_readonly_with_cancel(
                &logical, None,
            )
            .unwrap()
            .entries;
            let tree = Arc::new(crate::zip_tree::ZipTree::build(logical.clone(), entries));
            let rows = book_materialization::materialize_archive_level(
                &tree,
                &[],
                crate::settings::SortOrder::FileName,
                Some(&app.settings.grid_display_order),
            );
            (
                BookScopeKey::archive(&logical, &[]).unwrap(),
                PageDomain::ArchiveLevel {
                    prefix: String::new(),
                },
                BookCompositionSource::Archive {
                    tree,
                    prefix: vec![],
                    display_prefix: vec![],
                },
                ProjectionRecipe::Book(BookScope::Archive {
                    logical_root: logical,
                    prefix: vec![],
                }),
                rows,
            )
        }
        SourceKind::Pdf | SourceKind::Epub => {
            let logical = app.tmp.path().join(if matches!(kind, SourceKind::Epub) {
                "saved-book.epub"
            } else {
                "saved-book.pdf"
            });
            let physical = if matches!(kind, SourceKind::Epub) {
                let mut epub = zip::ZipWriter::new(std::fs::File::create(&logical).unwrap());
                epub.start_file("mimetype", zip::write::SimpleFileOptions::default())
                    .unwrap();
                epub.write_all(b"application/epub+zip").unwrap();
                epub.finish().unwrap();
                let generated = logical.with_extension("generated.pdf");
                write_pdf(&generated);
                pin = Some(crate::pdf_loader::pin_epub_with_direction_for_test(
                    &logical,
                    1242,
                    std::fs::metadata(&generated).unwrap().len(),
                    crate::pdf_loader::PdfReadingDirection::L2R,
                ));
                generated
            } else {
                write_pdf(&logical);
                logical.clone()
            };
            backend = Some(crate::pdf_loader::RemotePdfTestBackend::for_path(&physical));
            let read = crate::pdf_loader::resolve_read_target(&logical).unwrap();
            let (items, metas, _) =
                book_materialization::materialize_cached_document_rows(&logical, 4, 0, 0);
            (
                BookScopeKey::document(&logical).unwrap(),
                PageDomain::Document,
                BookCompositionSource::Document {
                    count: 4,
                    metadata: None,
                    physical_source: logical.clone(),
                    read_target: Some(read),
                },
                ProjectionRecipe::Book(BookScope::Document(logical)),
                (items, metas),
            )
        }
    };
    let owner = app.build_window_context_for_test(1242, |viewer| {
        viewer.current_folder = Some(scope.logical_root.clone());
        viewer.zip_nav = match &source {
            BookCompositionSource::Archive { tree, .. } => {
                Some(crate::zip_tree::ZipNavState::new(tree.clone()))
            }
            _ => None,
        };
        viewer.items = rows.0;
        viewer.image_metas = rows.1;
        viewer.thumbnails = vec![ThumbnailState::Pending; viewer.items.len()];
        viewer.visible_indices = (0..viewer.items.len()).collect();
        viewer.projection_recipe = recipe;
        viewer.spread_mode = crate::settings::SpreadMode::Ltr;
        viewer.reading_direction = crate::settings::ReadingDirection::Ltr;
        viewer.fullscreen_idx = Some(1);
        viewer.book_composition = ViewerBookComposition::Accepted {
            scope: scope.clone(),
            token: service.snapshot().token_for(&scope),
            record: None,
            layout: CompositionProjectionOutcome::Automatic,
            source,
            generation: viewer.items_generation,
            cause: super::super::book_composition_runtime::ProjectionAdoptionCause::Normal,
        };
    });
    Fixture {
        app,
        service,
        process: Some(process),
        scope,
        domain,
        owner,
        _pdf_backend: backend,
        _epub_pin: pin,
    }
}

fn prepare_editor(f: &mut Fixture, ctx: &egui::Context) {
    f.app
        .with_viewer_context(f.owner, |app| {
            assert!(app.start_local_composition_editor(ctx));
            let CompositionEditorState::Preparing { input, receiver } =
                std::mem::take(&mut app.composition_editor)
            else {
                panic!("editor preparation expected");
            };
            let completion = receiver.recv_timeout(WAIT).unwrap();
            let (tx, rx) = mpsc::channel();
            tx.send(completion).unwrap();
            app.composition_editor = CompositionEditorState::Preparing {
                input,
                receiver: rx,
            };
            app.poll_composition_editor_operations(ctx);
            let CompositionEditorState::Editing(session) = &app.composition_editor else {
                panic!("Editing expected")
            };
            assert!(session.prepared.is_ok(), "{:?}", session.prepared);
            app.apply_editor_ui_action(
                ctx,
                ui::EditorUiAction::SetHidden {
                    page: 1,
                    hidden: true,
                },
            );
        })
        .unwrap();
}

fn send_and_retire(f: &mut Fixture, ctx: &egui::Context) -> mpsc::Sender<()> {
    let (entered, release) = f.service.hold_worker_for_test();
    entered.recv_timeout(WAIT).unwrap();
    f.app
        .with_viewer_context(f.owner, |app| {
            super::tests::finish_editor_results(app);
            app.save_local_composition_editor(ctx, false);
        })
        .unwrap();
    assert!(f.app.composition_editor_writes_pending());
    f.app
        .retire_context(f.owner, "metadata_maintenance_sent_save", |_| ())
        .unwrap();
    assert!(!f.app.composition_editor_modal_active());
    release
}

fn buffer_completion(f: &mut Fixture, failure: bool) {
    let ticket = &mut f.app.composition_editor_operations.tickets[0];
    let reply = ticket.receiver.recv_timeout(WAIT).unwrap();
    assert_eq!(
        reply.status.is_err(),
        failure,
        "{:?}",
        reply.status.as_ref().err()
    );
    let (tx, rx) = mpsc::channel();
    tx.send(reply).unwrap();
    ticket.receiver = rx;
}

fn assert_resume_once(f: &mut Fixture, ctx: &egui::Context, failure: bool) {
    assert!(!f.app.composition_editor_writes_pending());
    let writer = f.app.book_resume_writer.as_ref().unwrap();
    let cutoff = writer.watermarks().admitted;
    assert_eq!(cutoff, u64::from(!failure));
    let reply = writer
        .read_position_after(&crate::path_key::normalize(&f.scope.logical_root), cutoff)
        .recv_timeout(WAIT)
        .unwrap();
    assert!(reply.status.is_ok());
    assert_eq!(reply.position.is_some(), !failure);
    f.app.poll_composition_editor_operations(ctx);
    assert_eq!(
        f.app
            .book_resume_writer
            .as_ref()
            .unwrap()
            .watermarks()
            .admitted,
        cutoff
    );
}

fn check_transfer(kind: SourceKind) {
    for entry in [
        transfer::Entry::Preview,
        transfer::Entry::Import,
        transfer::Entry::Export,
    ] {
        for failure in [false, true] {
            let mut f = fixture(kind);
            let ctx = egui::Context::default();
            prepare_editor(&mut f, &ctx);
            if failure {
                let lease = crate::book_composition::maintenance_at(f.app.tmp.path()).unwrap();
                crate::book_composition::initialize_database_at(
                    &f.app.tmp.path().join("book_composition.db"),
                )
                .unwrap();
                let db = rusqlite::Connection::open(f.app.tmp.path().join("book_composition.db"))
                    .unwrap();
                db.execute_batch("CREATE TRIGGER reject_save BEFORE INSERT ON book_compositions BEGIN SELECT RAISE(ABORT,'save fixture failure'); END").unwrap();
                drop(db);
                drop(lease);
                f.service
                    .prepare(f.scope.clone(), f.domain.clone())
                    .recv_timeout(WAIT)
                    .unwrap();
            }
            let main_items = f.app.items.clone();
            let main_generation = f.app.items_generation;
            let release = send_and_retire(&mut f, &ctx);
            transfer::open(&mut f.app, entry);
            transfer::advance(&mut f.app);
            assert!(
                transfer::waiting_without_worker(&f.app),
                "{kind:?} {entry:?}"
            );
            release.send(()).unwrap();
            buffer_completion(&mut f, failure);
            transfer::advance(&mut f.app);
            assert!(
                transfer::waiting_without_worker(&f.app),
                "completed receipt must still be harvested"
            );
            // Hold Record after its watermark publication, before pending--.
            // A warm position reply is available here, but the actor is still busy.
            let completion_pause = (!failure).then(|| {
                f.app
                    .book_resume_writer
                    .as_ref()
                    .unwrap()
                    .pause_before_next_completion_for_test()
            });
            f.app.poll_composition_editor_operations(&ctx);
            if let Some((entered, _)) = &completion_pause {
                entered.recv_timeout(WAIT).unwrap();
            }
            assert_resume_once(&mut f, &ctx, failure);
            if completion_pause.is_some() {
                assert!(f.app.book_resume_writer.as_ref().unwrap().is_busy());
                if !matches!(entry, transfer::Entry::Preview) {
                    transfer::advance(&mut f.app);
                    assert!(
                        transfer::waiting_without_worker(&f.app),
                        "{kind:?} {entry:?}: resume reply is not writer idle"
                    );
                }
            }
            // The resume assertions are complete. As in the Generic migration
            // test, release and drain/join the actual FIFO to isolate ticket
            // harvest from the independent writer-idle admission condition.
            drop(completion_pause);
            drop(f.app.book_resume_writer.take());
            assert_eq!(f.app.items, main_items);
            assert_eq!(f.app.items_generation, main_generation);
            assert!(matches!(
                f.app.composition_editor,
                CompositionEditorState::Closed
            ));
            transfer::advance(&mut f.app);
            assert!(
                !transfer::waiting_without_worker(&f.app),
                "{kind:?} {entry:?} failure={failure}: transfer admitted after harvest and resume drain; rename_writers_busy={}",
                f.app.rename_migration_writers_busy()
            );
            transfer::finish_worker(&mut f.app);
            let stored = f
                .service
                .prepare(f.scope.clone(), f.domain.clone())
                .recv_timeout(WAIT)
                .unwrap()
                .status
                .unwrap();
            assert_eq!(stored.is_none(), failure);
            if let Some(stored) = stored {
                assert_eq!(stored.composition.hidden_pages.len(), 1);
            }
        }
    }
}

#[test]
fn zip_save_retired_owner_precedes_metadata_preview_import_and_export() {
    check_transfer(SourceKind::Zip);
}

#[test]
fn pdf_save_retired_owner_precedes_metadata_preview_import_and_export() {
    check_transfer(SourceKind::Pdf);
}

#[test]
fn epub_save_retired_owner_precedes_metadata_preview_import_and_export() {
    check_transfer(SourceKind::Epub);
}

#[test]
fn zip_sent_save_does_not_block_unrelated_sidecar_writer_readiness() {
    let mut f = fixture(SourceKind::Zip);
    let ctx = egui::Context::default();
    prepare_editor(&mut f, &ctx);
    let release = send_and_retire(&mut f, &ctx);
    assert!(
        !f.app
            .metadata_transfer_writers_busy_for_sidecar_restore(false)
            .unwrap()
    );
    release.send(()).unwrap();
    buffer_completion(&mut f, false);
    f.app.poll_composition_editor_operations(&ctx);
    assert_resume_once(&mut f, &ctx, false);
}

#[test]
fn zip_sent_save_retired_owner_precedes_generic_migration_after_journal_ack() {
    let mut f = fixture(SourceKind::Zip);
    let ctx = egui::Context::default();
    prepare_editor(&mut f, &ctx);
    let release = send_and_retire(&mut f, &ctx);
    f.app.rename_migration_data_dir_override = Some(f.app.tmp.path().to_path_buf());
    f.app.rename_migration_journal_admission = crate::app::RenameMigrationJournalAdmission::Durable;
    let old = f.app.tmp.path().join("unrelated-old.jpg");
    let new = f.app.tmp.path().join("unrelated-new.jpg");
    f.app.rename_migration_queue.push_back(
        crate::rename_key_migration::PathMigrationJob::shell_rename(old, new, false),
    );
    f.app.persist_rename_migration_journal();
    f.app.flush_rename_migration_journal().unwrap();
    assert!(!f.app.rename_migration_writers_busy());
    f.app.try_start_next_rename_migration();
    assert!(f.app.rename_migration_in_flight.is_none());
    release.send(()).unwrap();
    buffer_completion(&mut f, false);
    f.app.try_start_next_rename_migration();
    assert!(
        f.app.rename_migration_in_flight.is_none(),
        "buffered receipt still owns save"
    );
    f.app.poll_composition_editor_operations(&ctx);
    assert_resume_once(&mut f, &ctx, false);
    // A resume read reply precedes that actor's pending-count decrement.
    // The resume assertions are complete; its existing Drop drains and joins
    // the actual FIFO so this test isolates the composition ticket boundary.
    drop(f.app.book_resume_writer.take());
    f.app.try_start_next_rename_migration();
    let Some(crate::app::RenameMigrationInFlight::Generic { job, rx }) =
        f.app.rename_migration_in_flight.take()
    else {
        panic!("Generic admitted after ticket harvest and durable journal ACK");
    };
    let report = rx.recv_timeout(WAIT).unwrap();
    assert!(!report.panicked);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let (tx, rx) = mpsc::channel();
    tx.send(report).unwrap();
    f.app.rename_migration_in_flight =
        Some(crate::app::RenameMigrationInFlight::Generic { job, rx });
    f.app.poll_rename_migration_pending(&ctx);
    f.app.flush_rename_migration_journal().unwrap();
    // The Generic worker was harvested; complete the existing Collection stage too.
    f.app.try_start_next_rename_migration();
    if let Some(crate::app::RenameMigrationInFlight::Collection { job, rx }) =
        f.app.rename_migration_in_flight.take()
    {
        let outcome = rx.recv_timeout(WAIT).unwrap();
        assert!(outcome.is_ok(), "{outcome:?}");
        let (tx, rx) = crossbeam_channel::bounded(1);
        tx.send(outcome).unwrap();
        f.app.rename_migration_in_flight =
            Some(crate::app::RenameMigrationInFlight::Collection { job, rx });
        f.app.poll_rename_migration_pending(&ctx);
    }
    assert!(f.app.rename_migration_in_flight.is_none());
    assert!(f.app.rename_migration_queue.is_empty());
    f.app.flush_rename_migration_journal().unwrap();
}
