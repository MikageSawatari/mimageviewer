//! Non-C5 dispatch preserves the entry's frame and target, including terminal recipes.
use super::*;
use crate::ui_main::AddressBarNav;

fn frame(
    ctx: &egui::Context,
    target: egui::ViewportId,
    run: impl FnOnce(),
) -> Vec<egui::ViewportId> {
    ctx.begin_pass(egui::RawInput {
        viewport_id: target,
        viewports: [
            (egui::ViewportId::ROOT, egui::ViewportInfo::default()),
            (
                target,
                egui::ViewportInfo {
                    parent: (target != egui::ViewportId::ROOT).then_some(egui::ViewportId::ROOT),
                    ..Default::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    });
    run();
    let output = ctx.end_pass();
    let targets = output
        .viewport_output
        .iter()
        .flat_map(|(target, viewport)| {
            viewport.commands.iter().filter_map(move |command| {
                matches!(command, egui::ViewportCommand::Focus).then_some(*target)
            })
        })
        .collect();
    output.drop_without_applying_deltas();
    targets
}

#[test]
fn section1853_close_focus_non_c5_kinds_preserve_dispatch_frame_target_and_count() {
    for child in [false, true] {
        for kind in ["bookmarks", "history", "collection", "everything", "rating"] {
            let mut app = crate::app::setup_app_for_test();
            app.active_quick_folder_slot = None;
            let ctx = egui::Context::default();
            ctx.set_embed_viewports(false);
            let target = if child {
                app.fullscreen_viewport_id()
            } else {
                egui::ViewportId::ROOT
            };
            let nav = match kind {
                "bookmarks" => AddressBarNav::Bookmarks,
                "history" => AddressBarNav::ReadingHistory(ReadingHistoryEntrance::Menu),
                "collection" => {
                    AddressBarNav::Collection(top_level_grid_view::CollectionGridRestore {
                        identity: top_level_grid_view::CollectionGridIdentity {
                            collection_id: crate::collection_store::CollectionId::new(),
                        },
                        revision_at_open: 0,
                        viewport_anchor: None,
                        return_to: None,
                    })
                }
                "everything" => AddressBarNav::EverythingResults,
                "rating" => AddressBarNav::RatingViewBack,
                _ => unreachable!(),
            };
            let origin = app.close_return_origin().unwrap();
            app.pending_return_to_parent.state =
                FullscreenCloseState::Terminal(Box::new(CloseReturn {
                    origin,
                    effect: CloseReturnEffect::Navigation {
                        nav,
                        select_after_load: None,
                    },
                }));
            assert_eq!(
                frame(&ctx, target, || {
                    app.route_fullscreen_close_entry_focus(
                        &ctx,
                        FullscreenCloseRequestOutcome::ReturnAccepted,
                        true,
                        "non-c5-test",
                    );
                    let nav = app
                        .take_pending_return_to_parent_nav_with_focus(&ctx)
                        .unwrap();
                    assert!(match (&nav, kind) {
                        (AddressBarNav::Bookmarks, "bookmarks")
                        | (AddressBarNav::ReadingHistory(_), "history")
                        | (AddressBarNav::Collection(_), "collection")
                        | (AddressBarNav::EverythingResults, "everything")
                        | (AddressBarNav::RatingViewBack, "rating") => true,
                        _ => false,
                    });
                    assert!(app.apply_fullscreen_close_nav_immediate_with_focus(&ctx, nav));
                    assert!(
                        app.take_pending_return_to_parent_nav_with_focus(&ctx)
                            .is_none()
                    );
                }),
                [target],
                "{kind}, child={child}"
            );
            assert!(
                frame(&ctx, egui::ViewportId::ROOT, || {
                    assert!(
                        app.take_pending_return_to_parent_nav_with_focus(&ctx)
                            .is_none()
                    );
                })
                .is_empty()
            );
        }
    }
}

#[test]
fn section1853_close_focus_live_bookmark_and_no_destination_preserve_entry_target_once() {
    for child in [false, true] {
        for bookmark in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            app.active_quick_folder_slot = None;
            let ctx = egui::Context::default();
            let target = if child {
                app.fullscreen_viewport_id()
            } else {
                egui::ViewportId::ROOT
            };
            let book = app.tmp.path().join("book.pdf");
            app.current_folder = bookmark.then_some(book.clone());
            app.fullscreen_idx = Some(0);
            app.items = vec![GridItem::PdfPage {
                pdf_path: book.clone(),
                page_num: 0,
                content_type: None,
            }];
            app.thumbnails = vec![ThumbnailState::Failed];
            app.image_metas = vec![None];
            app.visible_indices = vec![0];
            if bookmark {
                app.bookmark_view_state = Some(BookmarkViewState::Opening {
                    target: crate::bookmark_browser::BookmarkViewReturnTarget::Book(book),
                    grid: BookmarkViewReturnGridState {
                        row_keys: vec![],
                        selected_key: None,
                        opened_key: None,
                        scroll_offset_y: 0.0,
                    },
                });
            }
            assert_eq!(
                frame(&ctx, target, || {
                    let outcome = if bookmark {
                        app.handle_fullscreen_close_request()
                    } else {
                        // An accepted request whose destination disappeared before dispatch.
                        app.pending_return_to_parent.request_live();
                        FullscreenCloseRequestOutcome::ReturnAccepted
                    };
                    assert_eq!(outcome, FullscreenCloseRequestOutcome::ReturnAccepted);
                    app.route_fullscreen_close_entry_focus(
                        &ctx,
                        outcome,
                        true,
                        "live-dispatch-test",
                    );
                    let nav = app.take_pending_return_to_parent_nav_with_focus(&ctx);
                    if bookmark {
                        assert!(matches!(nav, Some(AddressBarNav::Bookmarks)));
                        assert!(
                            app.apply_fullscreen_close_nav_immediate_with_focus(&ctx, nav.unwrap())
                        );
                    } else {
                        assert!(nav.is_none());
                    }
                    assert!(
                        app.take_pending_return_to_parent_nav_with_focus(&ctx)
                            .is_none()
                    );
                }),
                [target]
            );
            assert_eq!(app.fullscreen_idx, None);
        }
    }
}

#[test]
fn section1853_close_focus_ignored_request_and_non_focus_entry_emit_nothing() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    for sends_focus in [false, true] {
        assert!(
            frame(&ctx, egui::ViewportId::ROOT, || {
                app.route_fullscreen_close_entry_focus(
                    &ctx,
                    FullscreenCloseRequestOutcome::Ignored,
                    sends_focus,
                    "ignored-test",
                );
                assert!(
                    app.take_pending_return_to_parent_nav_with_focus(&ctx)
                        .is_none()
                );
            })
            .is_empty()
        );
    }
    app.pending_return_to_parent.request_live();
    assert!(
        frame(&ctx, egui::ViewportId::ROOT, || {
            app.route_fullscreen_close_entry_focus(
                &ctx,
                FullscreenCloseRequestOutcome::ReturnAccepted,
                false,
                "no-focus-entry-test",
            );
            let _ = app.take_pending_return_to_parent_nav_with_focus(&ctx);
        })
        .is_empty()
    );
}
