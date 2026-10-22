use super::{
    ViewerContextId,
    book_structural_op::{BookStructuralOp, Completion},
    tests::phase_c_support::setup_app,
};
use crate::{
    book_scope::BookScopeKey,
    books::BookOpIntent,
    spread_shift::{ShiftOperation, SpreadShiftPageRef, SpreadShiftService},
};
use std::{path::Path, sync::mpsc, time::Duration};

fn operation() -> ShiftOperation {
    ShiftOperation {
        context_id: ViewerContextId::for_test(1),
        items_generation: 1,
        operation_sequence: 1,
    }
}

fn read(service: &SpreadShiftService, root: &Path) -> Option<SpreadShiftPageRef> {
    let reply = service
        .read_after(
            BookScopeKey::folder(root).unwrap(),
            service.watermarks().admitted,
        )
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(reply.status.is_ok(), "{:?}", reply.status);
    reply.row
}

#[test]
fn book_manager_rename_during_maintenance_lease_keeps_captured_root_and_anchor() {
    let mut app = setup_app();
    let books = app.tmp.path().join("books");
    let old = books.join("Old");
    let new = books.join("New");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("Page.JPG"), b"page").unwrap();
    let data = app.tmp.path().join("shift-data");
    let (service, owner) = SpreadShiftService::spawn_at(data.clone(), None).unwrap();
    let anchor = SpreadShiftPageRef::RelativePath("Page.JPG".into());
    service
        .set_shift(
            BookScopeKey::folder(&old).unwrap(),
            anchor.clone(),
            operation(),
        )
        .unwrap();
    assert_eq!(read(&service, &old), Some(anchor.clone()));
    app.spread_shift_service = Some(service.clone());
    let completed = crate::books::rename_book_at(&data, &books, "Old", "New").unwrap();
    // The completion must use its captured roots, even if settings changed meanwhile.
    app.settings.book_root = Some(app.tmp.path().join("other-books"));
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(completed)).unwrap();
    app.book_structural_op = Some(BookStructuralOp::running_for_test(
        rx,
        BookOpIntent::SourceMutation,
        Completion::Manager,
    ));
    let lease = crate::spread_shift::maintenance_at(&data).unwrap();
    let before = service.watermarks().admitted;
    app.poll_book_structural_op(&egui::Context::default());
    assert!(app.book_structural_op.is_none());
    assert_eq!(service.watermarks().admitted, before + 1);
    let pending = service.read_after(BookScopeKey::folder(&new).unwrap(), before + 1);
    assert!(matches!(pending.try_recv(), Err(mpsc::TryRecvError::Empty)));
    drop(lease);
    let adopted = pending.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(adopted.row, Some(anchor.clone()));
    assert!(adopted.status.is_ok());
    assert_eq!(adopted.completed_through, before + 1);
    assert_eq!(read(&service, &old), None);
    let scope = BookScopeKey::folder(&new).unwrap();
    let index = crate::spread_shift::ShiftIdentityIndex::prepare(
        &scope,
        [(
            crate::book_resume_db::PageIdentity::Image {
                path: new.join("page.jpg").to_string_lossy().into_owned(),
            },
            4,
        )],
    )
    .unwrap();
    assert_eq!(index.resolve(&anchor), Some(4));
    owner.shutdown_and_join();
}

#[test]
fn book_manager_delete_worker_purges_same_name_and_adoption_preserves_sibling() {
    let mut app = setup_app();
    let books = app.tmp.path().join("books");
    let root = books.join("Deleted");
    let sibling = books.join("Deleted-other");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Page.JPG"), b"page").unwrap();
    let data = app.tmp.path().join("shift-data");
    let (service, owner) = SpreadShiftService::spawn_at(data.clone(), None).unwrap();
    let anchor = SpreadShiftPageRef::RelativePath("Page.JPG".into());
    for path in [&root, &sibling] {
        service
            .set_shift(
                BookScopeKey::folder(path).unwrap(),
                anchor.clone(),
                operation(),
            )
            .unwrap();
    }
    assert_eq!(read(&service, &root), Some(anchor.clone()));
    app.spread_shift_service = Some(service.clone());
    let completed = crate::books::delete_book_at(&data, &books, "Deleted").unwrap();
    // The filesystem worker owns the purge. Releasing its maintenance lease
    // has already refreshed the service snapshot before UI adoption starts.
    assert_eq!(read(&service, &root), None);
    assert_eq!(read(&service, &sibling), Some(anchor.clone()));
    app.settings.book_root = Some(app.tmp.path().join("other-books"));
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(completed)).unwrap();
    app.book_structural_op = Some(BookStructuralOp::running_for_test(
        rx,
        BookOpIntent::Delete { path: root.clone() },
        Completion::Manager,
    ));
    let lease = crate::spread_shift::maintenance_at(&data).unwrap();
    let before = service.watermarks().admitted;
    app.poll_book_structural_op(&egui::Context::default());
    assert!(app.book_structural_op.is_none());
    assert_eq!(
        service.watermarks().admitted,
        before,
        "UI adoption must not purge twice"
    );
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Page.JPG"), b"new page").unwrap();
    let pending = service.read_after(BookScopeKey::folder(&root).unwrap(), before);
    assert!(matches!(pending.try_recv(), Err(mpsc::TryRecvError::Empty)));
    drop(lease);
    let adopted = pending.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(adopted.status.is_ok());
    assert_eq!(adopted.completed_through, before);
    assert_eq!(adopted.row, None);
    assert_eq!(read(&service, &sibling), Some(anchor));
    owner.shutdown_and_join();
    let (reopened, reopened_owner) = SpreadShiftService::spawn_at(data, None).unwrap();
    assert_eq!(read(&reopened, &root), None);
    reopened_owner.shutdown_and_join();
}

#[test]
fn single_page_hard_purge_retains_parent_book_shift() {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let root = files.path().join("Book");
    std::fs::create_dir(&root).unwrap();
    let page = root.join("Page.JPG");
    std::fs::write(&page, b"page").unwrap();
    let (service, owner) = SpreadShiftService::spawn_at(data.path().into(), None).unwrap();
    let anchor = SpreadShiftPageRef::RelativePath("Page.JPG".into());
    service
        .set_shift(
            BookScopeKey::folder(&root).unwrap(),
            anchor.clone(),
            operation(),
        )
        .unwrap();
    assert_eq!(read(&service, &root), Some(anchor.clone()));
    std::fs::remove_file(&page).unwrap();
    let purge = crate::rename_key_migration::purge_removed_scopes_at(
        data.path(),
        &[crate::delete_worker::DeleteSourceScope::Exact(page)],
        &[],
    );
    assert!(purge.errors.is_empty(), "{:?}", purge.errors);
    assert_eq!(read(&service, &root), Some(anchor));
    owner.shutdown_and_join();
}

#[test]
fn book_transfer_completion_clears_departed_composition_and_shift_before_name_reuse() {
    use crate::book_composition::{
        BookComposition, BookCompositionService, CompositionPageRef, PageDomain, PageOverride,
        PageOverrideEntry, ReadingDirection,
    };

    let mut app = setup_app();
    let books = app.tmp.path().join("books");
    let source = books.join("Source");
    let destination = books.join("Destination");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&destination).unwrap();
    let first = source.join("0001_same.jpg");
    let second = source.join("0002_same.jpg");
    std::fs::write(&first, b"first page").unwrap();
    std::fs::write(&second, b"second page").unwrap();
    std::fs::write(destination.join("0001_existing.jpg"), b"destination page").unwrap();
    let data = app.tmp.path().join("book-transfer-data");
    let (shift, shift_owner) = SpreadShiftService::spawn_at(data.clone(), None).unwrap();
    let (composition, composition_owner) =
        BookCompositionService::spawn_at(data.clone(), None).unwrap();
    let page = |name: &str| CompositionPageRef::RelativePath { path: name.into() };
    let mut source_composition = BookComposition::new(
        ReadingDirection::Rtl,
        vec![page("0001_same.jpg"), page("0002_same.jpg")],
    );
    source_composition.cover_enabled = true;
    source_composition.hidden_pages = vec![page("0001_same.jpg")];
    source_composition.page_overrides = vec![PageOverrideEntry {
        page: page("0001_same.jpg"),
        directive: PageOverride::Alone,
    }];
    let mut destination_composition =
        BookComposition::new(ReadingDirection::Ltr, vec![page("0001_existing.jpg")]);
    destination_composition.page_overrides = vec![PageOverrideEntry {
        page: page("0001_existing.jpg"),
        directive: PageOverride::Alone,
    }];
    for (root, value, anchor) in [
        (&source, source_composition, "0001_same.jpg"),
        (&destination, destination_composition, "0001_existing.jpg"),
    ] {
        let scope = BookScopeKey::folder(root).unwrap();
        let saved = composition
            .save(scope.clone(), PageDomain::ImageFolder, value)
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(saved.status.is_ok(), "{:?}", saved.status);
        shift
            .set_shift(
                scope,
                SpreadShiftPageRef::RelativePath(anchor.into()),
                operation(),
            )
            .unwrap();
        assert_eq!(
            read(&shift, root),
            Some(SpreadShiftPageRef::RelativePath(anchor.into()))
        );
    }
    let prepare = |root: &Path| {
        composition
            .prepare(BookScopeKey::folder(root).unwrap(), PageDomain::ImageFolder)
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .status
            .unwrap()
            .unwrap()
    };
    let before_source = prepare(&source);
    let before_destination = prepare(&destination);
    let before_destination_shift = read(&shift, &destination);
    app.spread_shift_service = Some(shift.clone());
    app.book_composition_service = Some(composition.clone());
    app.rename_migration_data_dir_override = Some(data.clone());

    // Use the actual filesystem worker result, including its final composed map:
    // the second image takes the departed first image's exact path.
    let completed = crate::books::transfer_pages_between_books_at(
        &data,
        books,
        source.clone(),
        vec![first.clone(), second.clone()],
        vec![first.clone()],
        "Destination".into(),
        crate::books::BookTransferKind::Move,
    )
    .unwrap();
    let crate::books::BookOpResult::Transfer(summary) = &completed else {
        panic!("expected transfer result");
    };
    assert!(
        summary.metadata.errors.is_empty(),
        "{:?}",
        summary.metadata.errors
    );
    assert_eq!(summary.source_entries.len(), 1);
    assert_eq!(summary.source_entries[0].path, first);
    assert_eq!(std::fs::read(&first).unwrap(), b"second page");
    assert!(!second.exists());
    assert_eq!(
        std::fs::read(destination.join("0002_same.jpg")).unwrap(),
        b"first page"
    );
    // The worker does not directly edit service-owned rows. Completion adoption
    // must enqueue each service's book-specific request with the complete map.
    assert_eq!(prepare(&source), before_source);
    assert_eq!(
        read(&shift, &source),
        Some(SpreadShiftPageRef::RelativePath("0001_same.jpg".into()))
    );
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(completed)).unwrap();
    app.book_structural_op = Some(BookStructuralOp::running_for_test(
        rx,
        BookOpIntent::SourceMutation,
        Completion::ReorderTransfer,
    ));
    app.poll_book_structural_op(&egui::Context::default());
    assert!(app.book_structural_op.is_none());
    let adopted = prepare(&source).composition;
    assert_eq!(adopted.page_order, vec![page("0001_same.jpg")]);
    assert!(adopted.hidden_pages.is_empty());
    assert!(adopted.page_overrides.is_empty());
    assert!(adopted.cover_enabled);
    assert_eq!(adopted.direction, ReadingDirection::Rtl);
    assert_eq!(read(&shift, &source), None);
    assert_eq!(prepare(&destination), before_destination);
    assert_eq!(read(&shift, &destination), before_destination_shift);
    assert!(shift.drain_errors().is_empty());
    assert!(composition.drain_errors().is_empty());
    composition_owner.shutdown_and_join();
    shift_owner.shutdown_and_join();
}
