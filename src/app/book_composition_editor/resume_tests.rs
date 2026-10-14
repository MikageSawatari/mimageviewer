use super::*;
use crate::app::book_composition_runtime::BookCompositionSource;
use crate::app::book_materialization::{MaterializedBookRows, project_document_composition};
use crate::grid_item::GridItem;

fn page(n: u32) -> CompositionPageRef {
    CompositionPageRef::PdfPage { page_num: n }
}
fn input() -> EditorSourceInput {
    let root = std::path::PathBuf::from("C:/fixture/book.epub");
    EditorSourceInput {
        scope: BookScopeKey::document(&root).unwrap(),
        domain: PageDomain::Document,
        source: BookCompositionSource::Document {
            count: 100,
            metadata: None,
            physical_source: root,
            read_target: None,
        },
        policy: Default::default(),
        placement: Default::default(),
        sort: crate::settings::SortOrder::FileName,
        rules: crate::spread_shift::ResolvedAutomaticRules {
            mode: crate::settings::SpreadMode::Ltr,
            direction: crate::settings::ReadingDirection::Ltr,
            flow: crate::settings::ReadingFlow::Paged,
            final_cover: false,
            page_alone: Default::default(),
            singleton_endpoints: Default::default(),
            landscape_epoch: 0,
        },
        shift: None,
        rotations: Default::default(),
        data_dir: "C:/fixture".into(),
        password: None,
        initial_binding: None,
        raw_executor: std::sync::Arc::new(crate::raw::RawDevelopExecutor::new(1).unwrap()),
    }
}
fn rows(input: &EditorSourceInput, hidden: &[u32]) -> MaterializedBookRows {
    let mut record = BookComposition::new(
        ReadingDirection::Ltr,
        (0..100)
            .map(|n| CompositionUnit::Single {
                page: page(n),
                placement: SinglePlacement::Center,
                companion: SingleCompanion::Vacant,
            })
            .collect(),
    );
    record.hidden_pages = hidden.iter().copied().map(page).collect();
    project_document_composition(
        &input.scope.logical_root,
        100,
        (
            (0..100)
                .map(|n| GridItem::PdfPage {
                    pdf_path: input.scope.logical_root.clone(),
                    page_num: n,
                    content_type: None,
                })
                .collect(),
            vec![None; 100],
        ),
        input.policy.clone(),
        CompositionToken {
            service_epoch: 1,
            publication_sequence: 1,
        },
        Some(&record),
    )
    .unwrap()
}
fn raw() -> Vec<CanonicalRawPage> {
    (0..100)
        .map(|n| CanonicalRawPage {
            page: page(n),
            raw_index: n as usize,
        })
        .collect()
}
fn intent(input: &EditorSourceInput, seed: ResumeSeed) -> ResumeWriteIntent {
    ResumeWriteIntent::Reanchor {
        owner: CompositionEditorOwner {
            context_id: ViewerContextId::for_test(0),
            scope: input.scope.clone(),
            request_id: 1,
        },
        resume_key: input.scope.logical_root.clone(),
        seed,
        old_order: (0..100)
            .map(|n| page(n).resume_identity(&input.scope.logical_root))
            .collect(),
    }
}
#[test]
fn sent_save_receipt_updates_shared_resume_at_most_once_and_uses_existing_dedup() {
    let mut app = crate::app::tests::phase_c_support::setup_app();
    app.book_resume_writer = Some(
        crate::book_resume_db::BookResumeWriter::spawn_at(app.tmp.path().join("editor-resume.db"))
            .unwrap(),
    );
    let input = input();
    let rows = rows(&input, &[19, 79]);
    let committed = resolve_editor_resume(
        &intent(
            &input,
            ResumeSeed::Page(page(19).resume_identity(&input.scope.logical_root)),
        ),
        &rows,
        &input,
        &raw(),
    )
    .unwrap()
    .unwrap();
    let owner = CompositionEditorOwner {
        context_id: app.projected_viewer_context_id(),
        scope: input.scope.clone(),
        request_id: 1,
    };
    let (tx, rx) = mpsc::channel();
    tx.send(SaveCompletion {
        status: Ok(Some(committed)),
    })
    .unwrap();
    app.composition_editor_operations
        .tickets
        .push(SentSaveTicket {
            owner,
            receiver: rx,
        });
    let ctx = egui::Context::default();
    app.poll_composition_editor_operations(&ctx);
    assert_eq!(
        app.book_resume_writer
            .as_ref()
            .unwrap()
            .watermarks()
            .admitted,
        1
    );
    app.poll_composition_editor_operations(&ctx);
    assert_eq!(
        app.book_resume_writer
            .as_ref()
            .unwrap()
            .watermarks()
            .admitted,
        1
    );
    app.persist_book_resume_with_identity(
        input.scope.logical_root.clone(),
        20,
        ReadingMeterValue::new(20, 98),
        Some(page(20).resume_identity(&input.scope.logical_root)),
    );
    assert_eq!(
        app.book_resume_writer
            .as_ref()
            .unwrap()
            .watermarks()
            .admitted,
        1
    );
    let reply = app
        .book_resume_writer
        .as_ref()
        .unwrap()
        .read_position_after(&crate::path_key::normalize(&input.scope.logical_root), 1)
        .recv_timeout(std::time::Duration::from_secs(15))
        .unwrap();
    assert_eq!(
        reply.position.unwrap().identity,
        Some(page(20).resume_identity(&input.scope.logical_root))
    );
}
#[test]
fn save_resume_owner_20_to_21_is_independent_of_sibling_80_to_81_resolution_order() {
    let input = input();
    let rows = rows(&input, &[19, 79]);
    let raw = raw();
    let a = page(19).resume_identity(&input.scope.logical_root);
    let b = page(79).resume_identity(&input.scope.logical_root);
    for sibling_first in [false, true] {
        let resolve_sibling = || {
            let at = find_editor_landing(
                &b,
                &(0..100)
                    .map(|n| page(n).resume_identity(&input.scope.logical_root))
                    .collect::<Vec<_>>(),
                &rows,
            )
            .unwrap();
            assert_eq!(
                PageIdentity::for_item(&rows.items[at], None),
                Some(page(80).resume_identity(&input.scope.logical_root))
            );
        };
        if sibling_first {
            resolve_sibling();
        }
        let committed = resolve_editor_resume(
            &intent(&input, ResumeSeed::Page(a.clone())),
            &rows,
            &input,
            &raw,
        )
        .unwrap()
        .unwrap();
        assert_eq!(committed.position.page, 20);
        assert_eq!(
            committed.position.identity,
            Some(page(20).resume_identity(&input.scope.logical_root))
        );
        assert_eq!(committed.meter, ReadingMeterValue::new(20, 98).unwrap());
        if !sibling_first {
            resolve_sibling();
        }
    }
}
#[test]
fn save_resume_keeps_absent_unsafe_or_all_hidden_seed_and_adapts_legacy_raw() {
    let input = input();
    let visible_rows = rows(&input, &[19]);
    let raw = raw();
    assert!(
        resolve_editor_resume(&ResumeWriteIntent::Keep, &visible_rows, &input, &raw)
            .unwrap()
            .is_none()
    );
    for identity in [
        PageIdentity::Document {
            container: "C:/other.epub".into(),
            page_num: 19,
        },
        page(999).resume_identity(&input.scope.logical_root),
    ] {
        assert!(
            resolve_editor_resume(
                &intent(&input, ResumeSeed::Page(identity)),
                &visible_rows,
                &input,
                &raw
            )
            .unwrap()
            .is_none()
        );
    }
    let all = rows(&input, &(0..100).collect::<Vec<_>>());
    assert!(
        resolve_editor_resume(
            &intent(
                &input,
                ResumeSeed::Page(page(19).resume_identity(&input.scope.logical_root))
            ),
            &all,
            &input,
            &raw
        )
        .unwrap()
        .is_none()
    );
    for existing in [
        None,
        Some(ReadingPosition {
            page: 19,
            identity: None,
        }),
        Some(ReadingPosition {
            page: 999,
            identity: None,
        }),
    ] {
        let (tx, rx) = mpsc::channel();
        let expected = existing.as_ref().is_some_and(|p| p.page == 19);
        tx.send(crate::book_resume_db::PositionReadReply {
            cutoff: 0,
            completed_through: 0,
            position: existing,
            status: Ok(()),
        })
        .unwrap();
        let committed = resolve_editor_resume(
            &intent(&input, ResumeSeed::Existing(rx)),
            &visible_rows,
            &input,
            &raw,
        )
        .unwrap();
        assert_eq!(committed.is_some(), expected);
        if let Some(committed) = committed {
            assert_eq!(
                committed.position.identity,
                Some(page(20).resume_identity(&input.scope.logical_root))
            );
        }
    }
}
