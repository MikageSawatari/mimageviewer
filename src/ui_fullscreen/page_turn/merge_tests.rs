//! Page-turn frame ownership composed with master rating membership (§1.371).
#![cfg(all(test, windows))]

use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{
    adopt_interrupted_target, draw_current, frame, ready_app, seed_leaf,
};

const ONLY_THREE: [bool; 6] = [false, false, false, true, false, false];

fn rated_book(ctx: &egui::Context, count: usize) -> crate::app::AppTestEnvForTest {
    let mut app = ready_app(ctx, SpreadMode::Single);
    let folder = app.tmp.path().join("merge-pages");
    std::fs::create_dir_all(&folder).unwrap();
    app.items = (0..count)
        .map(|idx| {
            let path = folder.join(format!("{idx}.png"));
            image::RgbaImage::from_pixel(80, 120, image::Rgba([80, 100, 140, 255]))
                .save(&path)
                .unwrap();
            GridItem::Image(path)
        })
        .collect();
    app.current_folder = Some(folder);
    app.visible_indices = (0..count).collect();
    app.thumbnails = vec![ThumbnailState::Pending; count];
    app.image_metas = vec![None; count];
    app.settings.rating_filter = ONLY_THREE;
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    // Rating rebasing reopens through the ordinary presentation owner. Keep
    // its persisted preference consistent with this embedded runtime fixture.
    app.settings.video_in_window_mode = true;
    app.native_video_in_window_active = true;
    app.viewer_presentation = crate::app::ViewerPresentation::MainWindow;
    app.selected = Some(0);
    for idx in 0..count {
        let key = app.rating_path_key(idx).unwrap();
        app.rating_db.as_ref().unwrap().set(&key, 3).unwrap();
        app.rating_cache.insert(idx, 3);
    }
    install(ctx);
    install_fs_navigator_input_tracking(ctx);
    app
}

fn pressed(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn input_frame(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    events: Vec<egui::Event>,
    run: impl FnMut(&egui::Context),
) {
    let mut raw = egui::RawInput {
        viewport_id: viewport,
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(600.0, 400.0),
        )),
        events,
        ..Default::default()
    };
    raw.viewports.insert(
        viewport,
        egui::ViewportInfo {
            focused: Some(true),
            ..Default::default()
        },
    );
    let _ = ctx.run_pass(raw, run).drop_without_applying_deltas();
}

#[test]
fn page_turn_merge_rating_key_rebases_root_forwarded_root_and_viewer_before_navigation() {
    let _serial = crate::key_input::lock_test_input();
    for route in 0..3 {
        let ctx = egui::Context::default();
        let root = egui::Context::default();
        let mut app = rated_book(&ctx, 4);
        let events = vec![pressed(egui::Key::F6), pressed(egui::Key::ArrowRight)];
        if route == 0 {
            app.settings.video_in_window_mode = false;
            app.native_video_in_window_active = false;
            app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
            input_frame(&ctx, egui::ViewportId::ROOT, events, |ctx| {
                assert!(app.handle_fullscreen_root_key_input(ctx));
            });
        } else if route == 1 {
            input_frame(&root, egui::ViewportId::ROOT, events, |root| {
                let destination = app.page_turn_destination();
                input_frame(
                    &ctx,
                    egui::ViewportId::from_hash_of("merge-forwarded"),
                    vec![],
                    |ctx| {
                        ctx.root_ui(|root_ui| {
                            app.render_fullscreen_viewport_with_root_input(
                                root_ui,
                                Some(FullscreenRootFrameInput {
                                    context: root.clone(),
                                    destination,
                                }),
                            )
                        });
                    },
                );
            });
        } else {
            input_frame(&ctx, egui::ViewportId::ROOT, events, |ctx| {
                ctx.root_ui(|root_ui| app.render_fullscreen_viewport(root_ui));
            });
        }
        assert_eq!(app.rating_cache.get(&0), Some(&0), "route={route}");
        assert_eq!(
            app.fullscreen_idx,
            Some(2),
            "rating lands on 1, then the accepted next moves to 2; route={route}, continuation={:?}, ready={:?}, blocked={}",
            app.page_turn_presentation.continuation_operation_for_test(),
            app.pending_fs_page_turn_ready
                .as_ref()
                .map(|p| p.composition.navigation_anchor_idx),
            app.fs_navigation_sequence_blocks_new_target(),
        );
        if route == 2 {
            // The first draw is not yet adopted when its footer dispatches
            // navigation, so animation eligibility is a separate contract.
            // Check the actual prepared and submitted page instead: an old
            // frame snapshot would paint 0 or dispatch the next back to 1.
            let ready = app.pending_fs_page_turn_ready.as_ref().unwrap();
            assert_eq!(ready.composition.navigation_anchor_idx, 1);
            assert_eq!(ready.sources.pages.len(), 1);
            assert_eq!(ready.sources.pages[0].occurrence.idx, 1);
            let FsDisplayUnitSubmission::Paged(receipt) = &ready.sources.submission else {
                panic!("fresh page must have the ordinary submission proof");
            };
            assert!(!receipt.curl);
        }
        crate::key_input::clear_test_frame();
    }
}

#[test]
fn page_turn_merge_rating_empty_exits_all_key_routes_and_preserves_rating_history() {
    let _serial = crate::key_input::lock_test_input();
    for route in 0..3 {
        let ctx = egui::Context::default();
        let root = egui::Context::default();
        let mut app = rated_book(&ctx, 1);
        let events = vec![pressed(egui::Key::F6), pressed(egui::Key::ArrowRight)];
        if route == 0 {
            app.settings.video_in_window_mode = false;
            app.native_video_in_window_active = false;
            app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
            input_frame(&ctx, egui::ViewportId::ROOT, events, |ctx| {
                assert!(app.handle_fullscreen_root_key_input(ctx));
            });
        } else if route == 1 {
            input_frame(&root, egui::ViewportId::ROOT, events, |root| {
                let destination = app.page_turn_destination();
                input_frame(
                    &ctx,
                    egui::ViewportId::from_hash_of("merge-empty"),
                    vec![],
                    |ctx| {
                        ctx.root_ui(|root_ui| {
                            app.render_fullscreen_viewport_with_root_input(
                                root_ui,
                                Some(FullscreenRootFrameInput {
                                    context: root.clone(),
                                    destination,
                                }),
                            )
                        });
                    },
                );
            });
        } else {
            input_frame(&ctx, egui::ViewportId::ROOT, events, |ctx| {
                ctx.root_ui(|root_ui| app.render_fullscreen_viewport(root_ui));
            });
        }
        assert_eq!(app.fullscreen_idx, None, "route={route}");
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Inactive
        ));
        assert!(app.pending_fs_page_turn_ready.is_none());
        assert_eq!(app.meta_undo.undo_len(), 1);
        assert!(app.fs_feedback_toast.is_some());
        crate::key_input::clear_test_frame();
    }
}

#[test]
fn page_turn_merge_rating_empty_retires_turning_and_completing_in_common_close() {
    for completing in [false, true] {
        let ctx = egui::Context::default();
        let mut app = rated_book(&ctx, 4);
        seed_leaf(&ctx, &mut app);
        let tracker = match &app.page_turn_presentation.state {
            PresentationState::Turning { offer, .. } => offer.submission.clone(),
            _ => panic!("seeded turn"),
        };
        if completing {
            frame(&ctx, |_, ctx| {
                app.handle_fs_navigation(
                    ctx,
                    false,
                    false,
                    None,
                    None,
                    None,
                    FsPageNav::Target(2),
                    None,
                    1,
                );
            });
            assert!(app.page_turn_completion_active());
        }
        if completing {
            // Capture the current canonical flat after navigation. Its service
            // prefix invalidates the preceding unit's receipt before selection.
            // Close must retire this fresh pending proof and the continuation.
            frame(&ctx, |ui, ctx| {
                let (composition, sources) = draw_current(&mut app, ui, ctx);
                app.emit_fs_page_turn_ready_for_display_unit(
                    ctx,
                    &composition,
                    FsPageTurnDecision::normal(),
                    &sources,
                );
            });
            assert!(app.pending_fs_page_turn_ready.is_some());
        } else {
            // Curl output is not a canonical-flat receipt. This branch verifies
            // cancellation/Inactive; pending cleanup is covered by Completing.
            assert!(app.pending_fs_page_turn_ready.is_none());
        }
        for idx in [0, 2, 3] {
            let key = app.rating_path_key(idx).unwrap();
            app.rating_db.as_ref().unwrap().set(&key, 0).unwrap();
            app.rating_cache.insert(idx, 0);
        }
        app.visible_indices = vec![1];
        assert!(app.apply_rating_edit_to_fullscreen_item(1, crate::keymap::RatingEdit::Assign(0)));
        assert_eq!(app.fullscreen_idx, None);
        assert_eq!(tracker.status(), SubmissionStatus::Cancelled);
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Inactive
        ));
        assert!(app.pending_fs_page_turn_ready.is_none());
        assert!(!app.commit_page_turn_continuation(&ctx));
        assert_eq!(app.meta_undo.undo_len(), 1);
        assert!(app.fs_feedback_toast.is_some());
    }
}

#[test]
fn page_turn_merge_manual_defer_cancels_rating_restore_before_the_flat_barrier() {
    use crate::rating_membership::RatingAdoptionIntent;
    let ctx = egui::Context::default();
    let mut app = rated_book(&ctx, 4);
    let origin = app.capture_rating_origin().unwrap();
    seed_leaf(&ctx, &mut app);
    let (_tx, rx) = std::sync::mpsc::channel();
    app.rating_view_pending = Some(crate::rating_view::RatingViewPending {
        stars: 3,
        sequence: 1,
        source_generation: app.items_generation,
        context: app.page_turn_destination().context,
        rating_write_generation: app.rating_session_write_generation,
        rating_preview: Default::default(),
        rating_adoption: RatingAdoptionIntent::RestoreOrigin(origin),
        sort: Default::default(),
        membership_only: true,
        navigation: None,
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
    });
    frame(&ctx, |_, ctx| {
        app.handle_fs_navigation(
            ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNav::Target(2),
            None,
            1,
        );
    });
    assert!(app.page_turn_completion_active());
    assert_eq!(app.fullscreen_idx, Some(1));
    let intent = app
        .rating_view_pending
        .as_ref()
        .unwrap()
        .rating_adoption
        .clone();
    assert!(matches!(intent, RatingAdoptionIntent::Reconcile));
    // A late membership adoption must use the retired intent and keep the
    // current page, rather than RestoreOrigin reopening page 0 mid-barrier.
    let before = app.capture_rating_membership();
    app.adopt_rating_membership(before, intent);
    assert_eq!(app.fullscreen_idx, Some(1));
    adopt_interrupted_target(&ctx, &mut app);
    frame(&ctx, |_, ctx| {
        app.begin_page_turn_viewer_frame(ctx, true, None);
    });
    assert_eq!(app.fullscreen_idx, Some(2));
}
