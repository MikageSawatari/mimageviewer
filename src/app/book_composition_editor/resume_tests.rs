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
    let mut record = BookComposition::new(ReadingDirection::Ltr, (0..100).map(page).collect());
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
        resume_key: crate::book_resume_key::BookResumeKey::legacy(&input.scope.logical_root),
        seed,
        old_order: (0..100)
            .map(|n| page(n).resume_identity(&input.scope.logical_root))
            .collect(),
    }
}

fn inner_archive_input() -> (EditorSourceInput, Vec<CanonicalRawPage>) {
    let mut input = input();
    let logical = std::path::PathBuf::from("C:/Books/outer.rar");
    let tree = std::sync::Arc::new(crate::zip_tree::ZipTree::build(
        "D:/Cache/backing.zip".into(),
        [
            "A/001.jpg",
            "A/002.jpg",
            "A/003.jpg",
            "A/004.jpg",
            "B/001.jpg",
        ]
        .into_iter()
        .map(|name| crate::zip_loader::ZipImageEntry {
            entry_name: name.into(),
            uncompressed_size: 0,
            mtime: 0,
            container_ancestry: Vec::new(),
        })
        .collect(),
    ));
    input.scope = BookScopeKey::archive(&logical, &["A".into()]).unwrap();
    input.domain = PageDomain::ArchiveLevel {
        prefix: input.scope.level_prefix.clone(),
    };
    input.source = BookCompositionSource::Archive {
        tree,
        prefix: vec!["A".into()],
        display_prefix: vec!["A".into()],
    };
    let raw = (1..=4)
        .enumerate()
        .map(|(raw_index, page)| CanonicalRawPage {
            page: CompositionPageRef::ArchiveEntry {
                entry_name: format!("A/{page:03}.jpg"),
            },
            raw_index,
        })
        .collect();
    (input, raw)
}

fn install_inner_archive(app: &mut App, input: &EditorSourceInput) {
    let BookCompositionSource::Archive { tree, prefix, .. } = &input.source else {
        unreachable!()
    };
    let mut nav = crate::zip_tree::ZipNavState::new(tree.clone());
    nav.enter("A/");
    let rows = preparation::project_record(input, None).unwrap();
    app.current_folder = Some(tree.zip_path.clone());
    app.archive_source_override = Some(input.scope.logical_root.clone());
    app.zip_nav = Some(nav);
    app.projection_recipe = crate::app::book_materialization::ProjectionRecipe::Book(
        crate::app::book_materialization::BookScope::Archive {
            logical_root: input.scope.logical_root.clone(),
            prefix: prefix.clone(),
        },
    );
    app.thumbnails = vec![crate::grid_item::ThumbnailState::Pending; rows.items.len()];
    app.visible_indices = (0..rows.items.len()).collect();
    app.page_identity_index = rows.page_identities;
    app.image_metas = rows.metas;
    app.items = rows.items;
}

fn inner_archive_identity(input: &EditorSourceInput, page: u32) -> PageIdentity {
    CompositionPageRef::ArchiveEntry {
        entry_name: format!("A/{page:03}.jpg"),
    }
    .resume_identity(&input.scope.logical_root)
}

fn reordered_inner_archive_rows(
    input: &EditorSourceInput,
    hidden: Option<u32>,
) -> MaterializedBookRows {
    let mut record = BookComposition::new(
        ReadingDirection::Ltr,
        [4, 3, 1, 2]
            .map(|page| CompositionPageRef::ArchiveEntry {
                entry_name: format!("A/{page:03}.jpg"),
            })
            .to_vec(),
    );
    record.hidden_pages = hidden
        .into_iter()
        .map(|page| CompositionPageRef::ArchiveEntry {
            entry_name: format!("A/{page:03}.jpg"),
        })
        .collect();
    preparation::project_record(input, Some(&record)).unwrap()
}

fn publish_inner_editor_resume(
    app: &mut App,
    owner: CompositionEditorOwner,
    committed: ResumeCommit,
) {
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
    app.poll_composition_editor_save_receipts(&egui::Context::default());
}

#[test]
fn section1675_inner_resume_editor_intents_capture_page_and_reanchor_reorder_or_hidden_identity() {
    use crate::book_resume_key::BookResumeKey;
    for hidden in [false, true] {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        app.book_resume_writer = Some(
            crate::book_resume_db::BookResumeWriter::spawn_at(
                app.tmp.path().join("inner-editor-resume.db"),
            )
            .unwrap(),
        );
        let (input, raw) = inner_archive_input();
        install_inner_archive(&mut app, &input);
        app.fullscreen_idx = Some(1);
        let key = app.current_book_resume_key().unwrap();
        assert_eq!(
            key,
            BookResumeKey::archive(&input.scope.logical_root, &["A".into()]).unwrap()
        );
        let root = BookResumeKey::legacy(app.current_folder.as_deref().unwrap());
        let sibling = BookResumeKey::archive(&input.scope.logical_root, &["B".into()]).unwrap();
        let writer = app.book_resume_writer.as_ref().unwrap();
        assert!(writer.record_key_identity(&root, 70, None, None));
        assert!(writer.record_key_identity(&sibling, 80, None, None));
        assert!(writer.record_key_identity(&key, 1, None, Some(inner_archive_identity(&input, 2))));
        let owner = CompositionEditorOwner {
            context_id: app.projected_viewer_context_id(),
            scope: input.scope.clone(),
            request_id: 1,
        };
        let intent = app.capture_editor_resume_intent(&owner);
        assert!(
            matches!(&intent, ResumeWriteIntent::Reanchor { resume_key, seed: ResumeSeed::Page(identity), .. }
            if resume_key == &key && identity == &inner_archive_identity(&input, 2))
        );
        let rows = reordered_inner_archive_rows(&input, hidden.then_some(2));
        let committed = resolve_editor_resume(&intent, &rows, &input, &raw)
            .unwrap()
            .unwrap();
        let expected_page = if hidden { 3 } else { 2 };
        assert_eq!(committed.key, key);
        assert_eq!(committed.position.page, expected_page as usize - 1);
        assert_eq!(
            committed.position.identity,
            Some(inner_archive_identity(&input, expected_page))
        );
        assert_eq!(
            committed.meter,
            ReadingMeterValue::new(if hidden { 2 } else { 4 }, if hidden { 3 } else { 4 }).unwrap()
        );
        publish_inner_editor_resume(&mut app, owner, committed);
        let writer = app.book_resume_writer.as_ref().unwrap();
        let cutoff = writer.watermarks().admitted;
        assert_eq!(cutoff, 4);
        let position = writer
            .read_position_after(&key.storage_key(), cutoff)
            .recv_timeout(std::time::Duration::from_secs(15))
            .unwrap()
            .position
            .unwrap();
        assert_eq!(
            position.identity,
            Some(inner_archive_identity(&input, expected_page))
        );
        for (unchanged, expected) in [(&root, 70), (&sibling, 80)] {
            assert_eq!(
                writer
                    .read_position_after(&unchanged.storage_key(), cutoff)
                    .recv_timeout(std::time::Duration::from_secs(15))
                    .unwrap()
                    .position
                    .unwrap()
                    .page,
                expected
            );
        }
        app.poll_composition_editor_save_receipts(&egui::Context::default());
        assert_eq!(
            app.book_resume_writer
                .as_ref()
                .unwrap()
                .watermarks()
                .admitted,
            cutoff
        );
    }
}

#[test]
fn section1675_inner_resume_editor_intents_read_inner_fifo_without_root_fallback() {
    use crate::book_resume_key::BookResumeKey;
    for saved_inner in [false, true] {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        app.book_resume_writer = Some(
            crate::book_resume_db::BookResumeWriter::spawn_at(
                app.tmp.path().join("inner-editor-fenced-resume.db"),
            )
            .unwrap(),
        );
        let (input, raw) = inner_archive_input();
        install_inner_archive(&mut app, &input);
        let key = app.current_book_resume_key().unwrap();
        let root = BookResumeKey::legacy(app.current_folder.as_deref().unwrap());
        let writer = app.book_resume_writer.as_ref().unwrap();
        assert!(writer.record_key_identity(
            &root,
            1,
            None,
            Some(inner_archive_identity(&input, 2))
        ));
        let pause = writer.pause_for_test();
        if saved_inner {
            assert!(writer.record_key_identity(
                &key,
                2,
                None,
                Some(inner_archive_identity(&input, 3))
            ));
        }
        let cutoff = writer.watermarks().admitted;
        let read = writer.read_position_after(&key.storage_key(), cutoff);
        pause.send(()).unwrap();
        let owner = CompositionEditorOwner {
            context_id: app.projected_viewer_context_id(),
            scope: input.scope.clone(),
            request_id: 2,
        };
        let intent = ResumeWriteIntent::Reanchor {
            owner: owner.clone(),
            resume_key: key.clone(),
            seed: ResumeSeed::Existing(read),
            old_order: (1..=4)
                .map(|page| inner_archive_identity(&input, page))
                .collect(),
        };
        let rows = reordered_inner_archive_rows(&input, Some(3));
        let committed = resolve_editor_resume(&intent, &rows, &input, &raw).unwrap();
        assert_eq!(committed.is_some(), saved_inner);
        if let Some(committed) = committed {
            assert_eq!(committed.key, key);
            assert_eq!(committed.position.page, 3);
            assert_eq!(
                committed.position.identity,
                Some(inner_archive_identity(&input, 4))
            );
            assert_eq!(committed.meter, ReadingMeterValue::new(1, 3).unwrap());
            publish_inner_editor_resume(&mut app, owner, committed);
        }
        let writer = app.book_resume_writer.as_ref().unwrap();
        let cutoff = writer.watermarks().admitted;
        let root_position = writer
            .read_position_after(&root.storage_key(), cutoff)
            .recv_timeout(std::time::Duration::from_secs(15))
            .unwrap()
            .position
            .unwrap();
        assert_eq!(root_position.page, 1);
        assert_eq!(
            root_position.identity,
            Some(inner_archive_identity(&input, 2))
        );
        let inner = writer
            .read_position_after(&key.storage_key(), cutoff)
            .recv_timeout(std::time::Duration::from_secs(15))
            .unwrap()
            .position;
        assert_eq!(inner.is_some(), saved_inner);
        if let Some(inner) = inner {
            assert_eq!(inner.identity, Some(inner_archive_identity(&input, 4)));
        }
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
