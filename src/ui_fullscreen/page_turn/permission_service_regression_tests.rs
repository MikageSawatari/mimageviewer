//! Real composition service and ordinary navigation/slideshow handler regression.
#![cfg(test)]
use super::*;
use crate::app::book_composition_runtime::{BookCompositionSource, ViewerBookComposition};
use crate::app::book_materialization::*;
use crate::book_composition::*;
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, frame, ready_app};

fn book_fixture(
    ctx: &egui::Context,
    zip: bool,
    composed: bool,
    mode: SpreadMode,
) -> (
    crate::app::AppTestEnvForTest,
    tempfile::TempDir,
    BookCompositionProcessOwner,
) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(if zip { "book.zip" } else { "book.pdf" });
    let (service, owner) = BookCompositionService::spawn_at(temp.path().into(), None).unwrap();
    let scope = if zip {
        crate::book_scope::BookScopeKey::archive(&root, &[]).unwrap()
    } else {
        crate::book_scope::BookScopeKey::document(&root).unwrap()
    };
    let domain = if zip {
        PageDomain::ArchiveLevel {
            prefix: String::new(),
        }
    } else {
        PageDomain::Document
    };
    let page = |n: u32| {
        if zip {
            CompositionPageRef::ArchiveEntry {
                entry_name: format!("{n}.png"),
            }
        } else {
            CompositionPageRef::PdfPage { page_num: n }
        }
    };
    if composed {
        // Explicit Alone first singleton followed by an automatic pair:
        // exercise common-builder ownership and immediate permission admission.
        let mut book = BookComposition::new(
            if mode.is_rtl() {
                crate::book_composition::ReadingDirection::Rtl
            } else {
                crate::book_composition::ReadingDirection::Ltr
            },
            (0..4).map(page).collect(),
        );
        book.cover_enabled = mode.has_cover();
        book.page_overrides = [0, 3]
            .into_iter()
            .map(|n| PageOverrideEntry {
                page: page(n),
                directive: PageOverride::Alone,
            })
            .collect();
        service
            .save(scope.clone(), domain.clone(), book)
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .status
            .unwrap();
    }
    let prepared = service
        .prepare(scope.clone(), domain)
        .recv_timeout(Duration::from_secs(30))
        .unwrap();
    let record = prepared.status.unwrap().map(Arc::new);
    let mut app = ready_app(ctx, mode);
    let policy = app.settings.file_type_visibility.clone();
    let (rows, source) = if zip {
        let tree = Arc::new(crate::zip_tree::ZipTree::build(
            root.clone(),
            (0..4)
                .map(|n| crate::zip_loader::ZipImageEntry {
                    entry_name: format!("{n}.png"),
                    uncompressed_size: 0,
                    mtime: 0,
                    container_ancestry: vec![],
                })
                .collect(),
        ));
        let rows = materialize_archive_level(&tree, &[], crate::app::BOOK_READING_PAGE_ORDER, None);
        let rows = project_archive_composition(
            &tree,
            &root,
            &[],
            rows,
            crate::app::BOOK_READING_PAGE_ORDER,
            None,
            policy,
            prepared.token,
            record.as_ref().map(|record| &record.composition),
        )
        .unwrap();
        app.zip_nav = Some(crate::zip_tree::ZipNavState::new(tree.clone()));
        (
            rows,
            BookCompositionSource::Archive {
                tree,
                prefix: vec![],
                display_prefix: vec![],
            },
        )
    } else {
        let items = (0..4).map(|n| document_page_item(&root, n)).collect();
        let rows = project_document_composition(
            &root,
            4,
            (items, vec![None; 4]),
            policy,
            prepared.token,
            record.as_ref().map(|record| &record.composition),
        )
        .unwrap();
        (
            rows,
            BookCompositionSource::Document {
                count: 4,
                metadata: None,
                physical_source: root.clone(),
                read_target: None,
            },
        )
    };
    app.items = rows.items;
    app.image_metas = rows.metas;
    app.page_identity_index = rows.page_identities;
    app.current_folder = Some(root.clone());
    app.projection_recipe = ProjectionRecipe::Book(if zip {
        BookScope::Archive {
            logical_root: root,
            prefix: vec![],
        }
    } else {
        BookScope::Document(root)
    });
    app.book_composition = ViewerBookComposition::Accepted {
        scope,
        token: prepared.token,
        record,
        layout: rows.composition,
        source,
        generation: app.items_generation,
        cause: Default::default(),
    };
    app.install_book_composition_runtime(service);
    app.viewer_navigation_caches.invalidate();
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.settings.slideshow_interval_secs = 3.0;
    app.thumbnails = vec![ThumbnailState::Pending; 4];
    install(ctx);
    install_fs_navigator_input_tracking(ctx);
    (app, temp, owner)
}

fn draw(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
    let (composition, sources) = draw_current(app, ui, ctx);
    app.emit_fs_page_turn_ready_for_display_unit(
        ctx,
        &composition,
        FsPageTurnDecision::normal(),
        &sources,
    );
    let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
        panic!("paged receipt")
    };
    receipt.curl
}

fn seed_outgoing(app: &mut App, ctx: &egui::Context) {
    // Acquire outgoing permission through the REAL service, then publish its
    // canonical paint receipt. No test-only Allowed installer grants the turn.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let demand = app
            .spread_display_composition_for_anchor(0)
            .presentation_demand(app.spread_mode, app.fullscreen_page_slice);
        app.retain_book_presentation_demand(&demand);
        if matches!(
            app.book_presentation_current(&demand),
            PresentationDecision::Allowed(_)
        ) {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    frame(ctx, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        assert!(!draw(app, ui, ctx));
    });
}

fn finish_turn(app: &mut App, ctx: &egui::Context) {
    frame(ctx, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        assert!(draw(app, ui, ctx));
    });
    let PresentationState::Turning { offer, .. } = &mut app.page_turn_presentation.state else {
        panic!("turn started")
    };
    offer.accepted_at -= offer.duration + Duration::from_millis(1);
    frame(ctx, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        assert!(!draw(app, ui, ctx));
    });
    frame(ctx, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        assert!(!draw(app, ui, ctx));
    });
}

#[test]
fn permission_service_handlers_pdf_zip_automatic_and_composed_turn_on_receipt() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for zip in [false, true] {
        for composed in [false, true] {
            for mode in [
                SpreadMode::Single,
                SpreadMode::Ltr,
                SpreadMode::Rtl,
                SpreadMode::LtrCover,
                SpreadMode::RtlCover,
            ] {
                let ctx = egui::Context::default();
                let (mut app, _temp, owner) = book_fixture(&ctx, zip, composed, mode);
                seed_outgoing(&mut app, &ctx);
                let forward = if !composed && mode.is_spread() && !mode.has_cover() {
                    2
                } else {
                    1
                };
                for target in [forward, 0] {
                    let current = app.fullscreen_idx.unwrap();
                    frame(&ctx, |ui, ctx| {
                        app.begin_page_turn_viewer_frame(ctx, false, None);
                        let operation = ViewerOperationId::Pointer {
                            batch: next_operation_batch(),
                            source: egui::ViewportId::ROOT,
                            ordinal: 0,
                        };
                        app.page_turn_external_operation(
                            ctx,
                            operation,
                            OperationPhase::Press,
                            "pointer",
                        );
                        app.handle_fs_navigation_with_operation(
                            ctx,
                            false,
                            false,
                            None,
                            None,
                            None,
                            FsPageNav::Target(target),
                            None,
                            current,
                            Some(operation),
                        );
                        assert_eq!(app.fullscreen_idx, Some(target), "permission was immediate");
                        assert!(!app.book_composition_permission.pending());
                        assert!(
                            draw(&mut app, ui, ctx),
                            "zip={zip} composed={composed} mode={mode:?} target={target}"
                        );
                    });
                    finish_turn(&mut app, &ctx);
                }
                // Repeated automatic advance traverses the same owner and ready
                // snapshot after ordinary forward/back navigation.
                app.slideshow_playing = true;
                app.slideshow_next_at = Instant::now();
                frame(&ctx, |ui, ctx| {
                    app.begin_page_turn_viewer_frame(ctx, false, None);
                    app.advance_slideshow(ctx, 0);
                    assert_eq!(app.fullscreen_idx, Some(forward));
                    assert!(!app.book_composition_permission.pending());
                    assert!(draw(&mut app, ui, ctx));
                });
                finish_turn(&mut app, &ctx);
                app.slideshow_playing = false;
                owner.shutdown_and_join();
            }
        }
    }
}

#[test]
fn permission_service_admitted_write_waits_without_late_turn_offer() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for zip in [false, true] {
        for slideshow in [false, true] {
            let ctx = egui::Context::default();
            let (mut app, _temp, owner) = book_fixture(&ctx, zip, false, SpreadMode::Single);
            seed_outgoing(&mut app, &ctx);
            let service = app.book_composition_service.clone().unwrap();
            let scope = app.book_composition.scope().unwrap().clone();
            let domain = if zip {
                PageDomain::ArchiveLevel {
                    prefix: String::new(),
                }
            } else {
                PageDomain::Document
            };
            let (entered, release) = service.hold_worker_for_test();
            entered.recv_timeout(Duration::from_secs(30)).unwrap();
            let write = service.remove(scope.clone(), domain.clone());
            assert!(service.current_candidate_snapshot().is_none());
            take_test_events();
            frame(&ctx, |_, ctx| {
                app.begin_page_turn_viewer_frame(ctx, false, None);
                if slideshow {
                    app.slideshow_playing = true;
                    app.advance_slideshow(ctx, 0);
                } else {
                    let operation = ViewerOperationId::Pointer {
                        batch: next_operation_batch(),
                        source: egui::ViewportId::ROOT,
                        ordinal: 0,
                    };
                    app.page_turn_external_operation(
                        ctx,
                        operation,
                        OperationPhase::Press,
                        "pointer",
                    );
                    app.handle_fs_navigation_with_operation(
                        ctx,
                        false,
                        false,
                        None,
                        None,
                        None,
                        FsPageNav::Target(1),
                        None,
                        0,
                        Some(operation),
                    );
                }
                assert_eq!(app.fullscreen_idx, Some(0));
                assert!(app.book_composition_permission.pending());
            });
            release.send(()).unwrap();
            write
                .recv_timeout(Duration::from_secs(30))
                .unwrap()
                .status
                .unwrap();
            service
                .prepare(scope, domain)
                .recv_timeout(Duration::from_secs(30))
                .unwrap()
                .status
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            while app.fullscreen_idx != Some(1) {
                app.poll_book_presentation_permission(&ctx);
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            frame(&ctx, |ui, ctx| {
                app.begin_page_turn_viewer_frame(ctx, false, None);
                assert!(!draw(&mut app, ui, ctx));
            });
            assert!(
                !take_test_events()
                    .iter()
                    .any(|(_, kind, _)| *kind == "start")
            );
            owner.shutdown_and_join();
        }
    }
}

#[test]
fn permission_service_immediate_proof_requires_current_source_identity_and_occurrence() {
    for zip in [false, true] {
        for invalid in [
            "generation",
            "token",
            "identity",
            "index",
            "source",
            "unknown",
            "hidden",
            "checking",
        ] {
            let ctx = egui::Context::default();
            let (mut app, _temp, owner) = book_fixture(&ctx, zip, false, SpreadMode::Single);
            let demand = app
                .spread_display_composition_for_anchor(1)
                .presentation_demand(app.spread_mode, app.fullscreen_page_slice);
            let mut held = Vec::new();
            match invalid {
                "generation" => {
                    if let ViewerBookComposition::Accepted { generation, .. } =
                        &mut app.book_composition
                    {
                        *generation += 1;
                    }
                }
                "token" => {
                    if let ViewerBookComposition::Accepted { token, .. } = &mut app.book_composition
                    {
                        token.publication_sequence += 1;
                    }
                }
                "identity" => match &mut app.items[1] {
                    GridItem::PdfPage { page_num, .. } => *page_num = 99,
                    GridItem::ZipImage { entry_name, .. } => *entry_name = "../unsafe.png".into(),
                    _ => unreachable!(),
                },
                "index" => app.page_identity_index = Default::default(),
                "source" => {
                    if let ViewerBookComposition::Accepted { source, .. } =
                        &mut app.book_composition
                    {
                        *source = BookCompositionSource::Unavailable;
                    }
                }
                "unknown" => app.book_composition = ViewerBookComposition::Automatic,
                "hidden" => app.install_hidden_presentation_for_test(&demand),
                "checking" => held = app.install_checking_presentation_for_test(&demand),
                _ => unreachable!(),
            }
            assert!(
                app.defer_book_composition_navigation_open(
                    1,
                    crate::app::HistoryTrigger::UserChosen,
                    crate::app::FsOpenMaterialization::Eager,
                    crate::fs_page_load_scheduler::FsPageLoadContract::Sequential,
                    crate::app::book_composition_permission::BookTargetPurpose::Next
                ),
                "{invalid}"
            );
            assert_eq!(app.fullscreen_idx, Some(0));
            assert!(
                app.book_composition_permission.pending(),
                "{invalid} keeps the existing async owner"
            );
            drop(held);
            owner.shutdown_and_join();
        }
    }
}
