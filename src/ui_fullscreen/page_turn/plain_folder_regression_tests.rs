//! Production composition service with scanned physical folders and keyboard navigation.
use super::*;
use crate::app::tests::plain_folder_spread::{configure, make_folder};
use crate::book_composition::{
    BookCompositionProcessOwner, BookCompositionService, PresentationDecision,
};
use crate::ui_fullscreen::page_turn_submission_tests::draw_current;

fn folder_fixture(
    ctx: &egui::Context,
    mode: SpreadMode,
    enabled: bool,
    as_book: bool,
    count: usize,
) -> (crate::app::AppTestEnvForTest, BookCompositionProcessOwner) {
    let mut app = crate::app::tests::phase_c_support::setup_app();
    configure(&mut app, true, as_book);
    app.settings.default_spread_mode = mode;
    app.settings.default_reading_flow = ReadingFlow::Paged;
    let folder = make_folder(app.tmp.path(), "keyboard-folder");
    for n in 1..=count {
        image::RgbImage::new(80, 120)
            .save(folder.join(format!("{n:03}.jpg")))
            .unwrap();
    }
    let (service, owner) =
        BookCompositionService::spawn_at(app.tmp.path().join("composition-data"), None).unwrap();
    app.install_book_composition_runtime(service);
    app.load_folder(folder.clone());
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.current_folder.as_ref(), Some(&folder));
    assert_eq!(app.items.len(), count);
    assert!(
        app.items
            .iter()
            .all(|item| matches!(item, GridItem::Image(_)))
    );
    assert_eq!(app.spread_mode, mode);
    app.reading_flow = ReadingFlow::Paged;
    app.fullscreen_idx = Some(0);
    app.settings.page_turn_animation_enabled = enabled;
    app.settings.page_turn_animation_duration_ms = 600;
    app.settings.final_cover_spread_enabled = false;
    for idx in 0..count {
        let pixels = Arc::new(egui::ColorImage::filled([80, 120], egui::Color32::GREEN));
        let tex = ctx.load_texture(
            format!("folder-source-{idx}"),
            pixels.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                pdf_rerender_failures: Vec::new(),
                pdf_metadata: None,
                tex,
                pixels,
                source_dims: Some([80, 120]),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.record_page_dims_for_spread(idx, (80, 120));
        app.record_page_layout_dims_for_spread(idx, (80, 120));
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
    }
    app.thumbnails = vec![ThumbnailState::Pending; count];
    install(ctx);
    install_fs_navigator_input_tracking(ctx);
    (app, owner)
}

fn frame(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    mut run: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    let _ = ctx
        .run_pass(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 400.0),
                )),
                events,
                focused: true,
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|root_ui| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(root_ui, |ui| run(ui, ctx));
                })
            },
        )
        .drop_without_applying_deltas();
}

fn key(key: egui::Key, pressed: bool) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn keyboard_next(app: &mut App, ctx: &egui::Context, from: usize) -> Option<ViewerOperationId> {
    app.begin_page_turn_viewer_frame(ctx, false, None);
    let mut operation = None;
    let is_spread = app.spread_mode.is_spread();
    let action = app.handle_fs_key_input_with_operation(ctx, from, is_spread, &mut operation);
    assert!(!action.page_nav.is_none());
    app.handle_fs_navigation_with_operation(
        ctx,
        action.close,
        action.close_to_page_list,
        action.ctrl_nav,
        action.sibling_nav,
        action.mouse_nav,
        action.page_nav,
        action.jump_to,
        from,
        operation,
    );
    operation
}

#[test]
fn plain_folder_service_keyboard_turn_accepts_starts_and_finishes_single_and_spread() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for as_book in [false, true] {
        for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
            #[cfg(windows)]
            crate::key_input::clear_test_frame();
            let ctx = egui::Context::default();
            let (mut app, owner) = folder_fixture(&ctx, mode, true, as_book, 8);
            let next_key = if mode.is_rtl() {
                egui::Key::ArrowLeft
            } else {
                egui::Key::ArrowRight
            };
            let target = if mode.is_spread() { 2 } else { 1 };
            // Resident sources are sufficient; ordinary paint prepares exact
            // finals before the input and retains only the outgoing episode.
            for anchor in [target, 0] {
                app.fullscreen_idx = Some(anchor);
                let demand = app.book_target_presentation_demand(anchor);
                app.retain_book_presentation_demand(&demand);
                frame(&ctx, Vec::new(), |ui, ctx| {
                    draw_current(&mut app, ui, ctx);
                });
            }
            let target_demand = app.book_target_presentation_demand(target);
            assert_eq!(
                app.book_presentation_current(&target_demand),
                PresentationDecision::Checking
            );
            take_test_events();
            let mut operation = None;
            let mut first_receipt = None;
            frame(
                &ctx,
                vec![key(next_key, true), key(next_key, false)],
                |ui, ctx| {
                    operation = keyboard_next(&mut app, ctx, 0);
                    assert_eq!(app.fullscreen_idx, Some(target));
                    let before_paint = take_test_events();
                    assert!(
                        before_paint.iter().any(|(_, kind, _)| *kind == "offered"),
                        "service-backed physical folder must accept the turn: {before_paint:?}"
                    );
                    assert!(!before_paint.iter().any(|(_, kind, reason)| *kind == "skip"
                        && *reason == "book_presentation_not_fully_allowed"));
                    let (_, sources) = draw_current(&mut app, ui, ctx);
                    let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                        panic!("paged output")
                    };
                    assert!(receipt.curl);
                    first_receipt = Some(receipt);
                },
            );
            assert!(first_receipt.unwrap().was_accepted());
            let PresentationState::Turning { offer, clock, .. } =
                &mut app.page_turn_presentation.state
            else {
                panic!("keyboard turn started")
            };
            assert_eq!(Some(offer.fact.operation_id), operation);
            assert_eq!(offer.submission.status(), SubmissionStatus::Started);
            let tracker = offer.submission.clone();
            *clock = MotionClock::Running {
                start_at: Instant::now() - offer.duration - Duration::from_millis(1),
            };
            for _ in 0..2 {
                frame(&ctx, Vec::new(), |ui, ctx| {
                    app.begin_page_turn_viewer_frame(ctx, false, None);
                    draw_current(&mut app, ui, ctx);
                });
            }
            assert_eq!(tracker.status(), SubmissionStatus::Finished);
            let events = take_test_events();
            for expected in ["start", "finish"] {
                assert!(
                    events
                        .iter()
                        .any(|(fact, kind, _)| Some(fact.operation_id) == operation
                            && *kind == expected),
                    "{events:?}"
                );
            }
            assert!(
                !events
                    .iter()
                    .any(|(_, kind, _)| matches!(*kind, "skip" | "cancel")),
                "{events:?}"
            );
            owner.shutdown_and_join();
        }
    }
}

#[test]
fn plain_folder_service_continuous_single_keeps_cold_metadata_bounded_to_admission() {
    let ctx = egui::Context::default();
    let (mut app, owner) = folder_fixture(&ctx, SpreadMode::Single, true, false, 40);
    app.reading_flow = ReadingFlow::Vertical;
    app.rotation_cache.clear();
    let (base, current) = app.continuous_reading_units_and_pos(0).unwrap();
    assert!(app.rotation_cache.is_empty());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0));
    for _ in 0..2 {
        app.permitted_continuous_reading_layout(&ctx, rect, &base, current, false)
            .unwrap();
    }
    let admitted = app.book_presentation_demand_snapshot();
    assert!(!admitted.is_empty());
    assert!(admitted.len() < app.items.len());
    assert!(
        !app.rotation_cache.is_empty(),
        "admitted real pages resolve metadata"
    );
    // Geometry can shrink the final demand after the estimated first pass;
    // metadata for an earlier admitted neighbor may remain cached legitimately.
    assert!(app.rotation_cache.len() <= VERTICAL_READING_MAX_VISIBLE_PAGES);
    let unadmitted = base
        .iter()
        .rev()
        .flat_map(|unit| unit.presentation_demand())
        .find(|occurrence| !admitted.contains(occurrence))
        .unwrap();
    assert!(!app.rotation_cache.contains_key(&unadmitted.item_index));
    assert_eq!(
        app.book_presentation_allowed(&[unadmitted]),
        PresentationDecision::Checking
    );
    assert_eq!(
        app.book_presentation_target_allowed(&[unadmitted]),
        PresentationDecision::Allowed(vec![true])
    );
    owner.shutdown_and_join();
}

fn target_phase(app: &App) -> FsNavigationTargetPhase {
    let sequence = app
        .fs_holdover_tex
        .as_ref()
        .and_then(FsHoldover::navigation_sequence)
        .unwrap();
    let FsNavigationSequenceTarget::Display(target) = &sequence.target else {
        panic!("display target")
    };
    target.phase.clone()
}

#[test]
fn plain_folder_service_on_off_target_readiness_requests_burst_and_load() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for enabled in [false, true] {
        #[cfg(windows)]
        crate::key_input::clear_test_frame();
        let ctx = egui::Context::default();
        let (mut app, owner) = folder_fixture(&ctx, SpreadMode::Single, enabled, false, 8);
        let _ = app.ensure_final_composite_texture(&ctx, 1).unwrap();
        assert!(app.begin_fs_page_navigation_sequence(&ctx, 0, 1, false));
        app.fullscreen_idx = Some(1);
        assert!(matches!(
            target_phase(&app),
            FsNavigationTargetPhase::Awaiting { .. }
        ));
        frame(&ctx, Vec::new(), |_, ctx| {
            app.fs_page_turn_decision_for_frame(ctx, 1);
        });
        assert!(
            matches!(
                target_phase(&app),
                FsNavigationTargetPhase::Ready {
                    presentation: FsNavigationPresentation::Materialized,
                    ..
                }
            ),
            "ON={enabled}: resident target readiness must advance before visible admission"
        );

        // A source-less target still enters the existing thumbnail/rendition
        // request owner even while its visible occurrence is not retained.
        app.fs_holdover_tex = None;
        app.fs_cache.remove(&4);
        app.requested.remove(&4);
        app.keep_set.remove(&4);
        assert!(app.begin_fs_page_navigation_sequence(&ctx, 1, 4, true));
        app.fullscreen_idx = Some(4);
        frame(&ctx, Vec::new(), |_, ctx| {
            app.fs_page_turn_decision_for_frame(ctx, 4);
        });
        assert!(
            app.keep_set.contains(&4),
            "ON={enabled}: target thumbnail demand"
        );
        assert!(
            app.requested.contains_key(&4),
            "ON={enabled}: target thumbnail request"
        );
        assert!(
            app.passthrough_last_call.contains_key(&4),
            "ON={enabled}: rendition requested"
        );
        assert!(matches!(
            target_phase(&app),
            FsNavigationTargetPhase::Awaiting { .. }
        ));
        let pixels = Arc::new(egui::ColorImage::filled([80, 120], egui::Color32::WHITE));
        app.thumbnails[4] = ThumbnailState::loaded_raster(
            ctx.load_texture(
                "folder-rendition",
                pixels.clone(),
                egui::TextureOptions::LINEAR,
            ),
            crate::thumb_loader::ThumbLoadOrigin::UpgradeableCache,
            false,
            120,
            crate::source_coverage::SourceCoverage::FullSource,
            Some((80, 120)),
            None,
        );
        app.thumb_pixels.insert(4, pixels);
        frame(&ctx, Vec::new(), |_, ctx| {
            app.fs_page_turn_decision_for_frame(ctx, 4);
        });
        assert!(
            matches!(
                target_phase(&app),
                FsNavigationTargetPhase::Ready {
                    presentation: FsNavigationPresentation::Rendition,
                    ..
                }
            ),
            "ON={enabled}: target rendition readiness"
        );

        // The newly reachable input-level probe must retire a stale burst when
        // the key has been released. No real desktop input is sampled.
        #[cfg(windows)]
        {
            let viewport = app.fullscreen_viewport_id();
            let burst_id = egui::Id::unique(("fs_page_turn_burst", viewport));
            let burst = FsPageTurnBurstState::default()
                .advance(app.items_generation, 2, 3)
                .advance(app.items_generation, 3, 4);
            ctx.data_mut(|data| data.insert_temp(burst_id, burst));
            frame(&ctx, Vec::new(), |_, ctx| {
                app.fs_page_turn_decision_for_frame(ctx, 4);
            });
            assert_eq!(
                ctx.data(|data| data.get_temp::<FsPageTurnBurstState>(burst_id)),
                Some(FsPageTurnBurstState::Idle)
            );
        }

        app.fs_holdover_tex = None;
        app.fullscreen_idx = Some(6);
        app.fs_cache.remove(&6);
        app.fs_pending.remove(&6);
        assert_eq!(
            app.fs_page_load_state(6),
            crate::app::FsPageLoadState::NeedsLoad
        );
        let demand = app.book_target_presentation_demand(6);
        assert_eq!(
            app.book_presentation_current(&demand),
            PresentationDecision::Checking
        );
        frame(&ctx, Vec::new(), |_, ctx| {
            app.prepare_viewer_target_for_frame(ctx, 6, &mut None);
        });
        assert_ne!(
            app.fs_page_load_state(6),
            crate::app::FsPageLoadState::NeedsLoad,
            "ON={enabled}: normal target load must run before visible admission"
        );
        owner.shutdown_and_join();
    }
}
