use super::{ViewerContextId, tests::phase_c_support::setup_app};
use crate::{
    book_scope::BookScopeKey,
    books::{BookOpIntent, BookOpPending},
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
    let completed = crate::books::rename_book(&books, "Old", "New").unwrap();
    // The completion must use its captured roots, even if settings changed meanwhile.
    app.settings.book_root = Some(app.tmp.path().join("other-books"));
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(completed)).unwrap();
    app.book_op_pending = Some(BookOpPending {
        rx,
        intent: BookOpIntent::SourceMutation,
    });
    let lease = crate::spread_shift::maintenance_at(&data).unwrap();
    let before = service.watermarks().admitted;
    app.poll_book_op_pending(&egui::Context::default());
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
fn book_manager_delete_during_maintenance_lease_purges_same_name_and_preserves_sibling() {
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
    let completed = crate::books::delete_book(&books, "Deleted").unwrap();
    app.settings.book_root = Some(app.tmp.path().join("other-books"));
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(completed)).unwrap();
    app.book_op_pending = Some(BookOpPending {
        rx,
        intent: BookOpIntent::Delete { path: root.clone() },
    });
    let lease = crate::spread_shift::maintenance_at(&data).unwrap();
    let before = service.watermarks().admitted;
    app.poll_book_op_pending(&egui::Context::default());
    assert_eq!(service.watermarks().admitted, before + 1);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Page.JPG"), b"new page").unwrap();
    let pending = service.read_after(BookScopeKey::folder(&root).unwrap(), before + 1);
    assert!(matches!(pending.try_recv(), Err(mpsc::TryRecvError::Empty)));
    drop(lease);
    let adopted = pending.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(adopted.status.is_ok());
    assert_eq!(adopted.completed_through, before + 1);
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
