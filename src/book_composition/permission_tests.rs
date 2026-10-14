#![cfg(test)]

use super::*;
use std::sync::Arc;

fn occurrence() -> PresentationOccurrence {
    PresentationOccurrence {
        item_index: 2,
        navigation_anchor: 2,
        role: PresentationRole::Navigation,
        slice: PresentationSlice::Full,
        slot: PresentationSlot::Center,
    }
}
fn document(root: &str, page_num: usize) -> CandidatePage {
    CandidatePage {
        identity: crate::book_resume_db::PageIdentity::Document {
            container: root.into(),
            page_num,
        },
        physical_archive: None,
        raw_tree: None,
    }
}

#[test]
fn empty_write_admission_ignores_unavailable_store_but_protected_resources_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("book_composition.db");
    let original = b"invalid composition database";
    std::fs::write(&path, original).unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let scope = crate::book_scope::BookScopeKey::document("C:/book.pdf").unwrap();
    assert!(
        service
            .prepare(scope.clone(), PageDomain::Document)
            .recv()
            .unwrap()
            .status
            .is_err()
    );
    let snapshot = service.snapshot();
    assert!(snapshot.status.is_err());
    assert!(
        CompositionWriteAdmission::default()
            .validate(&snapshot)
            .is_ok()
    );
    for admission in [
        CompositionWriteAdmission {
            targets: vec![CompositionTarget {
                scope: scope.clone(),
                page: CompositionPageRef::PdfPage { page_num: 0 },
            }],
            ..Default::default()
        },
        CompositionWriteAdmission {
            binding_scope: Some(scope.clone()),
            ..Default::default()
        },
        CompositionWriteAdmission {
            unsupported_scopes: vec![ScopePermissionToken {
                token: snapshot.token_for(&scope),
                scope: scope.clone(),
            }],
            ..Default::default()
        },
        CompositionWriteAdmission {
            unsupported_pages: vec![document("C:/book.pdf", 0).identity],
            ..Default::default()
        },
    ] {
        assert!(admission.validate(&snapshot).is_err());
    }
    // Ordinary folder trim keeps the existing worker even when this unrelated store is unreadable.
    service
        .set_view_trim_book(
            crate::spread_db::container_key_with_fallback(&temp.path().join("image-book"), &[]),
            crate::view_trim::ViewTrimBookState::default(),
            CompositionWriteAdmission::default(),
        )
        .recv()
        .unwrap()
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    owner.shutdown_and_join();
}

#[test]
fn candidate_worker_checks_only_identity_and_adopts_fresh_scope_publication() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let scope = crate::book_scope::BookScopeKey::document("C:/Books/book.epub").unwrap();
    service
        .prepare(scope.clone(), PageDomain::Document)
        .recv()
        .unwrap()
        .status
        .unwrap();
    let candidate = document("C:/Books/book.epub", 2);
    let correlation = CandidateCorrelation {
        context_id: 7,
        request_id: 9,
        items_generation: 4,
        occurrence: CandidateOccurrence::Presented(occurrence()),
        identity: candidate.identity.clone(),
    };
    let first = service
        .check_candidate(correlation.clone(), candidate.clone())
        .recv()
        .unwrap();
    assert_eq!(first.correlation, correlation);
    let first = first.status.unwrap();
    assert!(matches!(first, CandidatePermission::Allowed(Some(_))));
    let mut composition = BookComposition::new(
        ReadingDirection::Ltr,
        vec![CompositionPageRef::PdfPage { page_num: 2 }],
    );
    composition
        .hidden_pages
        .push(CompositionPageRef::PdfPage { page_num: 2 });
    service
        .save(scope.clone(), PageDomain::Document, composition)
        .recv()
        .unwrap()
        .status
        .unwrap();
    assert!(!first.is_current(&service.snapshot()));
    let second = service
        .check_candidate(correlation, candidate)
        .recv()
        .unwrap()
        .status
        .unwrap();
    assert!(matches!(second, CandidatePermission::Hidden(_)));
    assert!(second.is_current(&service.snapshot()));
    // A sibling's absence token and a document's physical page identity are independent.
    assert!(matches!(
        candidate_permission(&service.snapshot(), &document("D:/Books/book.epub", 2)).unwrap(),
        CandidatePermission::Allowed(_)
    ));
    assert!(matches!(
        candidate_permission(&service.snapshot(), &document("C:/Books/book.epub", 3)).unwrap(),
        CandidatePermission::Allowed(_)
    ));
    service
        .remove(scope, PageDomain::Document)
        .recv()
        .unwrap()
        .status
        .unwrap();
    assert!(!second.is_current(&service.snapshot()));
    assert!(matches!(
        candidate_permission(&service.snapshot(), &document("C:/Books/book.epub", 2)).unwrap(),
        CandidatePermission::Allowed(_)
    ));
    owner.shutdown_and_join();
}

#[test]
fn raw_tree_proves_nested_wrapped_level_and_keeps_logical_source() {
    let entries = vec![crate::zip_loader::ZipImageEntry {
        entry_name: "wrapper/chapter.zip/pages/a.jpg".into(),
        uncompressed_size: 0,
        mtime: 0,
        container_ancestry: vec![],
    }];
    let tree = Arc::new(crate::zip_tree::ZipTree::build(
        "C:/cache/converted.zip".into(),
        entries,
    ));
    let candidate = CandidatePage {
        identity: crate::book_resume_db::PageIdentity::Archive {
            container: "D:/Books/source.rar".into(),
            entry_name: "wrapper/chapter.zip/pages/a.jpg".into(),
        },
        physical_archive: Some("C:/cache/converted.zip".into()),
        raw_tree: Some(tree),
    };
    let (scope, domain, page) = resolve_candidate_scope(&candidate).unwrap();
    assert_eq!(
        scope.storage_key(),
        "d:/books/source.rar/wrapper/chapter.zip/pages"
    );
    assert_eq!(
        domain,
        PageDomain::ArchiveLevel {
            prefix: "wrapper/chapter.zip/pages".into()
        }
    );
    assert_eq!(
        page,
        CompositionPageRef::ArchiveEntry {
            entry_name: "wrapper/chapter.zip/pages/a.jpg".into()
        }
    );
}

#[test]
fn unsafe_or_unproven_archive_source_does_not_infer_scope_from_current_rows() {
    let tree = Arc::new(crate::zip_tree::ZipTree::build(
        "C:/book.zip".into(),
        vec![],
    ));
    for entry_name in ["../a.jpg", "missing.jpg"] {
        let candidate = CandidatePage {
            identity: crate::book_resume_db::PageIdentity::Archive {
                container: "C:/book.zip".into(),
                entry_name: entry_name.into(),
            },
            physical_archive: None,
            raw_tree: Some(tree.clone()),
        };
        assert!(resolve_candidate_scope(&candidate).is_err());
    }
}

#[test]
fn mutation_boundary_rechecks_latest_hidden_and_binding_without_changing_rows() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let scope = crate::book_scope::BookScopeKey::document("C:/book.pdf").unwrap();
    service
        .prepare(scope.clone(), PageDomain::Document)
        .recv()
        .unwrap()
        .status
        .unwrap();
    let admission = CompositionWriteAdmission {
        targets: vec![CompositionTarget {
            scope: scope.clone(),
            page: CompositionPageRef::PdfPage { page_num: 4 },
        }],
        binding_scope: None,
        ..Default::default()
    };
    assert!(admission.validate(&service.snapshot()).is_ok());
    let mut composition = BookComposition::new(
        ReadingDirection::Ltr,
        vec![CompositionPageRef::PdfPage { page_num: 4 }],
    );
    composition
        .hidden_pages
        .push(CompositionPageRef::PdfPage { page_num: 4 });
    service
        .save(scope.clone(), PageDomain::Document, composition)
        .recv()
        .unwrap()
        .status
        .unwrap();
    assert!(admission.validate(&service.snapshot()).is_err());
    assert!(
        CompositionWriteAdmission {
            targets: vec![],
            binding_scope: Some(scope.clone()),
            ..Default::default()
        }
        .validate(&service.snapshot())
        .is_err()
    );
    let token = service.snapshot().token_for(&scope);
    assert!(
        CompositionWriteAdmission::default()
            .validate(&service.snapshot())
            .is_ok()
    );
    assert_eq!(token, service.snapshot().token_for(&scope));
    owner.shutdown_and_join();
}

#[test]
fn view_only_worker_writes_preserve_composition_and_dormant_shift() {
    use crate::settings::{ReadingDirection as BindingDirection, ReadingFlow, SpreadMode};
    let temp = tempfile::tempdir().unwrap();
    let scope = crate::book_scope::BookScopeKey::document("C:/view-only.pdf").unwrap();
    {
        let mut db = crate::spread_db::SpreadDb::open_at(&temp.path().join("spread.db")).unwrap();
        db.set_mode_and_direction(
            &scope.logical_root,
            None,
            SpreadMode::RtlCover,
            BindingDirection::Rtl,
            (
                SpreadMode::Single,
                ReadingFlow::Paged,
                BindingDirection::Ltr,
            ),
        )
        .unwrap();
    }
    let (shifts, shift_owner) =
        crate::spread_shift::SpreadShiftService::spawn_at(temp.path().into(), None).unwrap();
    let cutoff = shifts
        .set_shift(
            scope.clone(),
            crate::spread_shift::SpreadShiftPageRef::DocumentPageIdentity(2),
            crate::spread_shift::ShiftOperation {
                context_id: crate::app::ViewerContextId::for_test(9),
                items_generation: 3,
                operation_sequence: 1,
            },
        )
        .unwrap();
    let before_shift = shifts
        .read_after(scope.clone(), cutoff)
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap();
    before_shift.status.clone().unwrap();
    let before_watermarks = shifts.watermarks();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    service
        .save(
            scope.clone(),
            PageDomain::Document,
            fixtures::four_page_book(ReadingDirection::Rtl),
        )
        .recv()
        .unwrap()
        .status
        .unwrap();
    let before = service.snapshot();
    service
        .set_view_mode(scope.clone(), SpreadMode::Single)
        .unwrap()
        .recv()
        .unwrap()
        .unwrap();
    service
        .set_view_flow(scope.clone(), SpreadMode::SplitLtr, ReadingFlow::Vertical)
        .unwrap()
        .recv()
        .unwrap()
        .unwrap();
    let after = service.snapshot();
    assert_eq!(before.token_for(&scope), after.token_for(&scope));
    assert_eq!(before.rows, after.rows);
    assert_eq!(shifts.watermarks(), before_watermarks);
    assert_eq!(
        shifts.read_after(scope.clone(), cutoff).recv().unwrap(),
        before_shift
    );
    let db = crate::spread_db::SpreadDb::open_at(&temp.path().join("spread.db")).unwrap();
    assert_eq!(db.get(&scope.logical_root), Some(SpreadMode::Single));
    assert_eq!(
        db.get_flow(&scope.logical_root),
        Some(ReadingFlow::Vertical)
    );
    assert_eq!(
        db.get_direction(&scope.logical_root),
        Some(BindingDirection::Rtl)
    );
    owner.shutdown_and_join();
    shift_owner.shutdown_and_join();
}

#[test]
fn superseded_projection_trim_batches_keep_submission_order_and_drain_on_exit() {
    use crate::view_trim::{ViewTrimMargins, ViewTrimPageOverride};
    use crate::view_trim_db::{ViewTrimDb, ViewTrimWriteBatch};
    let temp = tempfile::tempdir().unwrap();
    let scope = crate::book_scope::BookScopeKey::document("C:/trim-fifo.pdf").unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    service
        .prepare(scope.clone(), PageDomain::Document)
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    let before = service.snapshot();
    let page_key = "c:/trim-fifo.pdf::page0";
    let older = ViewTrimPageOverride::from_margins(ViewTrimMargins {
        left: 0.05,
        ..Default::default()
    });
    let newer = ViewTrimPageOverride::from_margins(ViewTrimMargins {
        right: 0.12,
        ..Default::default()
    });
    let old_completion = service.save_view_trim(ViewTrimWriteBatch {
        book: None,
        pages: vec![(page_key.into(), older)],
    });
    // Superseding the projection drops its completion owner; the accepted write remains FIFO.
    drop(old_completion);
    let new_completion = service.save_view_trim(ViewTrimWriteBatch {
        book: None,
        pages: vec![(page_key.into(), newer)],
    });
    // Exit must finish accepted writes even when the projection workers no longer own receipts.
    owner.shutdown_and_join();
    new_completion
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .unwrap();
    let db = ViewTrimDb::open_at(&temp.path().join("view_trim.db")).unwrap();
    assert_eq!(
        db.load_page_overrides_many(&[page_key]).get(page_key),
        Some(&newer)
    );
    let after = service.snapshot();
    assert_eq!(before.rows, after.rows);
    assert_eq!(before.token_for(&scope), after.token_for(&scope));
    let rejected = service.save_view_trim(ViewTrimWriteBatch {
        book: None,
        pages: vec![(page_key.into(), older)],
    });
    assert!(
        rejected
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap()
            .is_err()
    );
    assert_eq!(
        db.load_page_overrides_many(&[page_key]).get(page_key),
        Some(&newer)
    );
}

#[test]
fn ordinary_trim_edit_and_reset_follow_older_projection_batch_in_same_fifo() {
    use crate::view_trim::{ViewTrimMargins, ViewTrimPageOverride};
    use crate::view_trim_db::{ViewTrimDb, ViewTrimWriteBatch};
    let temp = tempfile::tempdir().unwrap();
    let scope = crate::book_scope::BookScopeKey::document("C:/trim-reset-fifo.pdf").unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    service
        .prepare(scope.clone(), PageDomain::Document)
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    let before = service.snapshot();
    let page_key = "c:/trim-reset-fifo.pdf::page0";
    let batch = |margin| ViewTrimWriteBatch {
        book: None,
        pages: vec![(
            page_key.into(),
            ViewTrimPageOverride::from_margins(ViewTrimMargins {
                left: margin,
                ..Default::default()
            }),
        )],
    };
    let superseded_projection = service.save_view_trim(batch(0.05));
    drop(superseded_projection);
    let ordinary_edit = service.save_view_trim(batch(0.12));
    let ordinary_reset = service.remove_view_trim_page(page_key.into());
    owner.shutdown_and_join();
    ordinary_edit
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .unwrap();
    ordinary_reset
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .unwrap();
    let db = ViewTrimDb::open_at(&temp.path().join("view_trim.db")).unwrap();
    assert!(
        db.load_page_overrides_many(&[page_key]).is_empty(),
        "an older projection cannot recreate a page override after the user's reset"
    );
    let after = service.snapshot();
    assert_eq!(before.rows, after.rows);
    assert_eq!(before.token_for(&scope), after.token_for(&scope));
    assert!(
        service
            .remove_view_trim_page(page_key.into())
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap()
            .is_err()
    );
}

#[test]
fn queued_trim_admission_fences_page_edit_readers_until_each_worker_receipt() {
    use crate::page_edit_write_epoch::{EditWriteEpoch, PAGE_EDIT_WRITES, TestEpochScope};
    use crate::view_trim::{ViewTrimMargins, ViewTrimPageOverride};
    use crate::view_trim_db::ViewTrimWriteBatch;
    let _epoch = TestEpochScope::fresh();
    let temp = tempfile::tempdir().unwrap();
    let scope = crate::book_scope::BookScopeKey::document("C:/trim-admission.pdf").unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    service
        .prepare(scope, PageDomain::Document)
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    let before = PAGE_EDIT_WRITES.sample();
    assert_eq!(before.active_writers, 0);
    let (entered, release) = service.hold_worker_for_test();
    entered
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap();
    let key = "c:/trim-admission.pdf::page0";
    let write = service.save_view_trim(ViewTrimWriteBatch {
        book: None,
        pages: vec![(
            key.into(),
            ViewTrimPageOverride::from_margins(ViewTrimMargins {
                top: 0.1,
                ..Default::default()
            }),
        )],
    });
    assert_eq!(
        PAGE_EDIT_WRITES.sample().active_writers,
        1,
        "accepted queued write already excludes a quiescent page-edit read"
    );
    let remove = service.remove_view_trim_page(key.into());
    let queued = PAGE_EDIT_WRITES.sample();
    assert_eq!(
        queued.active_writers, 2,
        "overlapping accepted commands own counted guards"
    );
    assert!(!EditWriteEpoch::read_is_stable(before, queued));
    release.send(()).unwrap();
    write
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .unwrap();
    remove
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .unwrap();
    let completed = PAGE_EDIT_WRITES.sample();
    assert_eq!(
        completed.active_writers, 0,
        "receipt publication follows admission and DB guard release"
    );
    assert_eq!(
        completed.completed_writes,
        before.completed_writes + 4,
        "both admission guards and existing nested DB guards publish their completion"
    );
    assert!(!EditWriteEpoch::read_is_stable(before, completed));
    assert!(EditWriteEpoch::read_is_stable(
        completed,
        PAGE_EDIT_WRITES.sample()
    ));
    owner.shutdown_and_join();
}

#[test]
fn output_check_is_fresh_and_unrelated_archive_never_requires_source_enumeration() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let scope = crate::book_scope::BookScopeKey::document("C:/output.pdf").unwrap();
    service
        .prepare(scope.clone(), PageDomain::Document)
        .recv()
        .unwrap()
        .status
        .unwrap();
    let check = CompositionOutputCheck::new(service.clone(), document("C:/output.pdf", 1));
    assert!(check.permits().unwrap());
    let mut book = BookComposition::new(
        ReadingDirection::Ltr,
        vec![CompositionPageRef::PdfPage { page_num: 1 }],
    );
    book.hidden_pages = book.page_order.clone();
    service
        .save(scope.clone(), PageDomain::Document, book)
        .recv()
        .unwrap()
        .status
        .unwrap();
    assert!(!check.permits().unwrap());
    let unrelated = CompositionOutputCheck::new(
        service.clone(),
        CandidatePage {
            identity: crate::book_resume_db::PageIdentity::Archive {
                container: "C:/does-not-exist.zip".into(),
                entry_name: "page.jpg".into(),
            },
            physical_archive: None,
            raw_tree: None,
        },
    );
    assert!(unrelated.permits().unwrap());
    service
        .remove(scope, PageDomain::Document)
        .recv()
        .unwrap()
        .status
        .unwrap();
    assert!(check.permits().unwrap());
    assert!(unrelated.permits().unwrap());
    owner.shutdown_and_join();
}

#[test]
fn output_check_unavailable_store_is_error_and_unsupported_source_keeps_ordinary_output() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) =
        BookCompositionService::spawn_at(temp.path().join("unreadable"), None).unwrap();
    let scope = crate::book_scope::BookScopeKey::document("C:/output.pdf").unwrap();
    service
        .prepare(scope, PageDomain::Document)
        .recv()
        .unwrap()
        .status
        .unwrap();
    // Put an unrelated record in the snapshot so the unsupported archive path
    // reaches the shared resolver only when that logical root has a saved scope.
    let root = "C:/unsupported.zip";
    let scope = crate::book_scope::BookScopeKey::archive(root, &[]).unwrap();
    service
        .save(
            scope,
            PageDomain::ArchiveLevel {
                prefix: String::new(),
            },
            BookComposition::new(
                ReadingDirection::Ltr,
                vec![CompositionPageRef::ArchiveEntry {
                    entry_name: "a.jpg".into(),
                }],
            ),
        )
        .recv()
        .unwrap()
        .status
        .unwrap();
    let tree = crate::zip_tree::ZipTree::build(
        root.into(),
        vec![crate::zip_loader::ZipImageEntry {
            entry_name: "a.jpg".into(),
            uncompressed_size: 0,
            mtime: 0,
            container_ancestry: vec![],
        }],
    );
    let unsupported = CompositionOutputCheck::new(
        service.clone(),
        CandidatePage {
            identity: crate::book_resume_db::PageIdentity::Archive {
                container: root.into(),
                entry_name: "../bad.jpg".into(),
            },
            physical_archive: None,
            raw_tree: Some(Arc::new(tree)),
        },
    );
    assert!(unsupported.permits().unwrap());
    owner.shutdown_and_join();
    let broken = temp.path().join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(
        broken.join("book_composition.db"),
        b"keep unreadable original",
    )
    .unwrap();
    let (service, owner) = BookCompositionService::spawn_at(broken.clone(), None).unwrap();
    service
        .prepare(
            crate::book_scope::BookScopeKey::document("C:/output.pdf").unwrap(),
            PageDomain::Document,
        )
        .recv()
        .unwrap();
    assert!(
        CompositionOutputCheck::new(service, document("C:/output.pdf", 0))
            .permits()
            .is_err()
    );
    owner.shutdown_and_join();
    assert_eq!(
        std::fs::read(broken.join("book_composition.db")).unwrap(),
        b"keep unreadable original"
    );
}

#[test]
fn output_run_prepares_large_converted_archive_once_and_refreshes_hidden_by_scope_token() {
    const COUNT: u32 = 10_000;
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let logical = "D:/Books/source.rar";
    let physical = std::path::PathBuf::from("C:/cache/converted.zip");
    let prefix = "wrapper/chapter.zip/pages";
    let page = |number| CompositionPageRef::ArchiveEntry {
        entry_name: format!("{prefix}/{number}.jpg"),
    };
    let tree = Arc::new(crate::zip_tree::ZipTree::build(
        physical.clone(),
        (0..COUNT)
            .map(|number| crate::zip_loader::ZipImageEntry {
                entry_name: format!("{prefix}/{number}.jpg"),
                uncompressed_size: 0,
                mtime: 0,
                container_ancestry: vec![],
            })
            .collect(),
    ));
    let scope = crate::book_scope::BookScopeKey::new(logical, prefix).unwrap();
    let domain = PageDomain::ArchiveLevel {
        prefix: prefix.into(),
    };
    let mut record = BookComposition::new(ReadingDirection::Ltr, (0..COUNT).map(page).collect());
    record.hidden_pages = (0..COUNT)
        .filter(|number| number % 2 == 0)
        .map(page)
        .collect();
    service
        .save(scope.clone(), domain.clone(), record.clone())
        .recv()
        .unwrap()
        .status
        .unwrap();
    let check = |number| {
        CompositionOutputCheck::new(
            service.clone(),
            CandidatePage {
                identity: crate::book_resume_db::PageIdentity::Archive {
                    container: logical.into(),
                    entry_name: format!("{prefix}/{number}.jpg"),
                },
                physical_archive: Some(physical.clone()),
                raw_tree: Some(Arc::clone(&tree)),
            },
        )
    };
    let mut resolver = CompositionOutputResolver::default();
    for number in (0..COUNT).rev() {
        assert_eq!(
            check(number).permits_with(&mut resolver).unwrap(),
            number % 2 != 0
        );
    }
    assert_eq!(resolver.prepared_sources, 1);
    assert_eq!(resolver.visited_pages, COUNT as usize);
    record.hidden_pages.clear();
    service
        .save(scope.clone(), domain.clone(), record)
        .recv()
        .unwrap()
        .status
        .unwrap();
    assert!(check(0).permits_with(&mut resolver).unwrap());
    assert_eq!(resolver.prepared_sources, 1);
    assert_eq!(resolver.visited_pages, COUNT as usize);
    service
        .remove(scope, domain)
        .recv()
        .unwrap()
        .status
        .unwrap();
    assert!(check(0).permits_with(&mut resolver).unwrap());
    owner.shutdown_and_join();
}

#[test]
fn output_run_does_not_share_scope_index_between_captured_context_raw_trees() {
    let temp = tempfile::tempdir().unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let root = "C:/same.zip";
    let scope = crate::book_scope::BookScopeKey::archive(root, &[]).unwrap();
    let mut record = BookComposition::new(
        ReadingDirection::Ltr,
        vec![CompositionPageRef::ArchiveEntry {
            entry_name: "0.jpg".into(),
        }],
    );
    record.hidden_pages = record.page_order.clone();
    service
        .save(
            scope,
            PageDomain::ArchiveLevel {
                prefix: String::new(),
            },
            record,
        )
        .recv()
        .unwrap()
        .status
        .unwrap();
    let tree = |number| {
        Arc::new(crate::zip_tree::ZipTree::build(
            root.into(),
            vec![crate::zip_loader::ZipImageEntry {
                entry_name: format!("{number}.jpg"),
                uncompressed_size: 0,
                mtime: 0,
                container_ancestry: vec![],
            }],
        ))
    };
    let first_tree = tree(0);
    let second_tree = tree(1);
    let check = |tree: Arc<crate::zip_tree::ZipTree>| {
        CompositionOutputCheck::new(
            service.clone(),
            CandidatePage {
                identity: crate::book_resume_db::PageIdentity::Archive {
                    container: root.into(),
                    entry_name: "0.jpg".into(),
                },
                physical_archive: None,
                raw_tree: Some(tree),
            },
        )
    };
    let mut resolver = CompositionOutputResolver::default();
    assert!(!check(first_tree).permits_with(&mut resolver).unwrap());
    assert!(check(second_tree).permits_with(&mut resolver).is_err());
    assert_eq!(resolver.prepared_sources, 2);
    assert_eq!(resolver.visited_pages, 2);
    owner.shutdown_and_join();
}

#[test]
fn output_source_acceptance_reads_publication_after_archive_index_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source.zip");
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&root).unwrap());
    writer
        .start_file("0.jpg", zip::write::SimpleFileOptions::default())
        .unwrap();
    std::io::Write::write_all(&mut writer, b"real selected source").unwrap();
    writer.finish().unwrap();
    let (service, owner) =
        BookCompositionService::spawn_at(temp.path().join("data"), None).unwrap();
    let scope = crate::book_scope::BookScopeKey::archive(&root, &[]).unwrap();
    let domain = PageDomain::ArchiveLevel {
        prefix: String::new(),
    };
    let page = CompositionPageRef::ArchiveEntry {
        entry_name: "0.jpg".into(),
    };
    let record = BookComposition::new(ReadingDirection::Ltr, vec![page.clone()]);
    service
        .save(scope.clone(), domain.clone(), record.clone())
        .recv()
        .unwrap()
        .status
        .unwrap();
    let initial_snapshot = service.snapshot();
    let candidate = CandidatePage {
        identity: crate::book_resume_db::PageIdentity::Archive {
            container: crate::path_key::normalize_keep_drive(&root),
            entry_name: "0.jpg".into(),
        },
        physical_archive: Some(root.clone()),
        raw_tree: None,
    };
    let mut resolver = CompositionOutputResolver::default();
    // Model an editor commit during physical source preparation: the closure is
    // called at source acceptance, after the disk-backed index has been prepared.
    assert!(
        !resolver
            .permission(&initial_snapshot, &candidate, || {
                let mut hidden = record.clone();
                hidden.hidden_pages.push(page.clone());
                service
                    .save(scope.clone(), domain.clone(), hidden)
                    .recv()
                    .unwrap()
                    .status
                    .unwrap();
                service.snapshot()
            })
            .unwrap()
    );
    assert_eq!(resolver.prepared_sources, 1);
    assert_eq!(resolver.visited_pages, 1);
    // The same acceptance boundary applies when source preparation is cached.
    assert!(
        resolver
            .permission(&initial_snapshot, &candidate, || {
                service
                    .save(scope.clone(), domain.clone(), record)
                    .recv()
                    .unwrap()
                    .status
                    .unwrap();
                service.snapshot()
            })
            .unwrap()
    );
    assert_eq!(resolver.prepared_sources, 1);
    owner.shutdown_and_join();
}

#[test]
fn current_permission_snapshot_waits_for_initialization_mutations_maintenance_and_exit() {
    let temp = tempfile::tempdir().unwrap();
    let lease = maintenance_at(temp.path()).unwrap();
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    assert!(service.current_candidate_snapshot().is_none());
    drop(lease);
    let scope = crate::book_scope::BookScopeKey::document("C:/current-permission.pdf").unwrap();
    service
        .prepare(scope.clone(), PageDomain::Document)
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    assert!(service.current_candidate_snapshot().is_some());
    for operation in ["remove", "mode", "flow", "maintain"] {
        let (entered, release) = service.hold_worker_for_test();
        entered
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap();
        match operation {
            "remove" => {
                let _ = service.remove(scope.clone(), PageDomain::Document);
            }
            "mode" => {
                let _ = service
                    .set_view_mode(scope.clone(), crate::settings::SpreadMode::Single)
                    .unwrap();
            }
            "flow" => {
                let _ = service
                    .set_view_flow(
                        scope.clone(),
                        crate::settings::SpreadMode::Single,
                        crate::settings::ReadingFlow::Paged,
                    )
                    .unwrap();
            }
            _ => service
                .purge_book_root(std::path::Path::new("C:/irrelevant-folder"))
                .unwrap(),
        }
        assert!(
            service.current_candidate_snapshot().is_none(),
            "{operation} admission precedes the check"
        );
        release.send(()).unwrap();
        service
            .prepare(scope.clone(), PageDomain::Document)
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap()
            .status
            .unwrap();
        assert!(
            service.current_candidate_snapshot().is_some(),
            "{operation} retires its admission"
        );
    }
    let lease = maintenance_at(temp.path()).unwrap();
    assert!(service.current_candidate_snapshot().is_none());
    let pending = service.prepare(scope, PageDomain::Document);
    assert!(matches!(
        pending.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    drop(lease);
    pending
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    assert!(service.current_candidate_snapshot().is_some());
    service.close_admission();
    assert!(service.current_candidate_snapshot().is_none());
    owner.shutdown_and_join();
}
