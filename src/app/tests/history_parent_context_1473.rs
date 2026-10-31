//! §1.473: typed return payloads follow their viewer bundle, never a sibling.
#![cfg(all(test, windows))]

use super::phase_c_support::setup_app;
use super::*;
use crate::app::top_level_grid_view::{
    RatingRootRestore, ReadingHistoryExit, ReadingHistoryRootRestore, ReadingHistoryVisit,
    TopLevelGridOrigin, TopLevelGridRestore, TopLevelGridSurface,
};
use crate::pdf_loader::{JobPriority, PdfRenderValidity, TestPdfQueue};

fn origin(restore: TopLevelGridRestore, route_path: &Path) -> Box<TopLevelGridOrigin> {
    Box::new(TopLevelGridOrigin {
        restore,
        route: FacetRoute::root(FacetScope::path(route_path)),
    })
}

fn nested_history_origin(base: &Path, serial: u64) -> Box<TopLevelGridOrigin> {
    let first = ReadingHistoryRootRestore {
        visit: ReadingHistoryVisit {
            context: serial,
            surface_generation: 11,
            switch_sequence: 2,
        },
        exit: ReadingHistoryExit::RecordedEntrance(origin(
            TopLevelGridRestore::Folder(base.join("library")),
            &base.join("library"),
        )),
    };
    let rating = origin(
        TopLevelGridRestore::Rating(RatingRootRestore {
            stars: 3,
            return_to: Some(origin(
                TopLevelGridRestore::ReadingHistory(first),
                &base.join("H1"),
            )),
        }),
        &base.join("rating"),
    );
    origin(
        TopLevelGridRestore::ReadingHistory(ReadingHistoryRootRestore {
            visit: ReadingHistoryVisit {
                context: serial,
                surface_generation: 23,
                switch_sequence: 4,
            },
            exit: ReadingHistoryExit::RecordedEntrance(rating),
        }),
        &base.join("H2"),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Returns {
    history: Option<ReadingHistoryBookReturn>,
    rating: Option<Box<TopLevelGridOrigin>>,
    rating_chain: Vec<PathBuf>,
}

fn returns(app: &App) -> Returns {
    Returns {
        history: app.reading_history_return_from.clone(),
        rating: app.rating_view_return_to.clone(),
        rating_chain: app.rating_view_nav_stack.clone(),
    }
}

fn install_physical(app: &mut App, pdf: &Path, return_to: Box<TopLevelGridOrigin>, rating: bool) {
    app.current_folder = Some(pdf.to_path_buf());
    app.items = vec![GridItem::PdfPage {
        pdf_path: pdf.to_path_buf(),
        page_num: 0,
        content_type: None,
    }];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.image_metas = vec![None];
    app.visible_indices = vec![0];
    app.fullscreen_idx = Some(0);
    if rating {
        app.rating_view_stars = 3;
        app.rating_view_nav_stack = vec![pdf.to_path_buf()];
        app.rating_view_return_to = Some(return_to);
        app.reading_history_return_from = None;
    } else {
        app.reading_history_return_from = Some(ReadingHistoryBookReturn {
            container: pdf.to_path_buf(),
            return_to,
        });
        app.rating_view_return_to = None;
        app.rating_view_nav_stack.clear();
    }
}

#[test]
fn section1473_context_mount_swap_and_retire_keep_complete_return_owners_separate() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let main_pdf = app.tmp.path().join("same.pdf");
    let main_origin = nested_history_origin(app.tmp.path(), 14730);
    install_physical(&mut app, &main_pdf, main_origin, false);
    let main = returns(&app);
    let main_ticket = app.pdf_render_epoch.snapshot();
    let sibling_origin = nested_history_origin(&app.tmp.path().join("sibling"), 14731);
    let sibling = app.build_window_context_for_test(14731, |owner| {
        install_physical(owner, &main_pdf, sibling_origin, true);
    });
    let (other, sibling_ticket) = app
        .with_viewer_context(sibling, |owner| {
            (returns(owner), owner.pdf_render_epoch.snapshot())
        })
        .unwrap();
    assert_ne!(main, other);
    for _ in 0..3 {
        app.with_viewer_context_state(sibling, |owner| {
            assert_eq!(returns(owner), other);
            assert!(
                owner
                    .pdf_render_epoch
                    .snapshot()
                    .same_request(&sibling_ticket)
            );
        })
        .unwrap();
        assert_eq!(returns(&app), main);
        assert!(app.pdf_render_epoch.snapshot().same_request(&main_ticket));
    }
    app.with_viewer_context(sibling, |owner| {
        owner.rating_view_return_to = None;
        owner.rating_view_nav_stack.clear();
    })
    .unwrap();
    assert_eq!(
        returns(&app),
        main,
        "consuming one mounted return must not consume main's nested payload"
    );
    app.retire_context(sibling, "section1473_return_owner_retire", |_| ())
        .unwrap();
    assert!(sibling_ticket.is_stale());
    assert!(!main_ticket.is_stale());
    assert_eq!(returns(&app), main);
    assert!(app.with_viewer_context(sibling, |_| ()).is_err());
}

#[test]
fn section1473_context_stash_and_materialized_fork_preserve_typed_returns() {
    for rating in [false, true] {
        let mut app = setup_app();
        app.active_quick_folder_slot = None;
        let pdf = app.tmp.path().join("same.pdf");
        let payload = nested_history_origin(app.tmp.path(), 14732);
        install_physical(&mut app, &pdf, payload, rating);
        let expected = returns(&app);
        let expected_close_origin = format!("{:?}", app.close_return_origin());
        let source_ticket = app.pdf_render_epoch.snapshot();
        let fork = app.fork_materialized_still_context(&pdf, 0);
        assert_eq!(returns(&app), expected);
        let fork_ticket = app
            .with_viewer_context(fork, |owner| {
                if rating {
                    // The native Rating chain stays in main; the independent reader
                    // inherits its immutable close recipe through the existing slot.
                    assert_eq!(
                        returns(owner),
                        Returns {
                            history: None,
                            rating: None,
                            rating_chain: vec![],
                        }
                    );
                } else {
                    assert_eq!(returns(owner), expected);
                }
                assert_eq!(
                    format!("{:?}", owner.close_return_origin()),
                    expected_close_origin,
                    "the inherited recipe includes the whole H2 → Rating → H1 → origin payload"
                );
                assert!(owner.reading_history_root_exit_nav().is_none());
                owner.pdf_render_epoch.snapshot()
            })
            .unwrap();
        assert!(!fork_ticket.same_request(&source_ticket));
        app.retire_context(fork, "section1473_return_fork_retire", |_| ())
            .unwrap();
        assert!(fork_ticket.is_stale());
        assert!(!source_ticket.is_stale());
        assert_eq!(returns(&app), expected);

        let previous_main = app.stash_mounted_and_start_fresh("section1473_return_owner_stash");
        assert_eq!(
            returns(&app),
            Returns {
                history: None,
                rating: None,
                rating_chain: vec![]
            }
        );
        app.with_viewer_context(previous_main, |owner| {
            assert_eq!(returns(owner), expected);
            assert!(
                owner
                    .pdf_render_epoch
                    .snapshot()
                    .same_request(&source_ticket)
            );
        })
        .unwrap();
        app.retire_context(previous_main, "section1473_stashed_return_retire", |_| ())
            .unwrap();
        assert!(source_ticket.is_stale());
        assert!(app.reading_history_return_from.is_none());
        assert!(app.rating_view_return_to.is_none());
    }
}

#[test]
fn section1473_main_root_exit_keeps_same_pdf_sibling_ticket_generation_and_queued_work() {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let pdf = app.tmp.path().join("same.pdf");
    let payload = nested_history_origin(app.tmp.path(), 14733);
    let sibling = app.build_window_context_for_test(14733, |owner| {
        install_physical(owner, &pdf, payload, false);
    });
    let (sibling_ticket, generation, cancel, before_returns) = app
        .with_viewer_context(sibling, |owner| {
            (
                owner.pdf_render_epoch.snapshot(),
                owner.items_generation,
                owner.cancel_token.clone(),
                returns(owner),
            )
        })
        .unwrap();
    let sibling_id = sibling;
    let root = ReadingHistoryRootRestore::unavailable();
    app.current_folder = Some(reading_history_synthetic_path());
    app.items_are_reading_history_view = true;
    app.top_level_grid_view
        .replace_surface(TopLevelGridSurface::ReadingHistory(root.clone()));
    let main_ticket = app.pdf_render_epoch.snapshot();
    let mut queue = TestPdfQueue::new();
    let main_reply = queue.enqueue(
        "same.pdf#0",
        PdfRenderValidity::Viewer(main_ticket.clone()),
        JobPriority::Normal,
    );
    let sibling_reply = queue.enqueue(
        "same.pdf#0",
        PdfRenderValidity::Viewer(sibling_ticket.clone()),
        JobPriority::Normal,
    );
    let nav = app
        .handle_gamepad_grid_back()
        .expect("root parent must produce an exit intent");
    assert!(matches!(
        nav,
        crate::ui_main::AddressBarNav::ReadingHistoryRootExit
    ));
    assert!(
        !sibling_ticket.is_stale(),
        "preview and intent may not invalidate sibling work"
    );
    assert!(app.apply_fullscreen_close_nav_immediate(nav));
    assert!(app.items_are_drive_list);
    assert!(
        main_ticket.is_stale(),
        "main retires only its own old PDF epoch"
    );
    assert!(!sibling_ticket.is_stale());
    app.with_viewer_context(sibling_id, |owner| {
        assert_eq!(owner.current_folder.as_ref(), Some(&pdf));
        assert_eq!(owner.items_generation, generation);
        assert_eq!(returns(owner), before_returns);
        assert!(Arc::ptr_eq(&owner.cancel_token, &cancel));
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(
            owner
                .pdf_render_epoch
                .snapshot()
                .same_request(&sibling_ticket)
        );
    })
    .unwrap();
    queue.prune_stale();
    assert!(queue.complete_next());
    sibling_reply
        .recv()
        .expect("the independent PDF job remains runnable after main's root exit");
    assert!(
        main_reply.recv().is_err(),
        "main alone retires its old PDF epoch"
    );
    app.retire_context(sibling_id, "section1473_pdf_sibling_retire", |_| ())
        .unwrap();
    assert!(sibling_ticket.is_stale());
}
