//! Crop materials must complete the real navigation/presentation lifecycle, including a page
//! whose requested view contains only its underlay. Receipts are produced by ordinary drawing.

use super::*;
use crate::source_coverage::{EditPreviewGeometry, SourceCoverage};
use crate::ui_fullscreen::page_turn_submission_tests::ready_app;

fn frame(
    ctx: &egui::Context,
    press_next: bool,
    mut run: impl FnMut(&mut egui::Ui, &egui::Context),
) -> egui::FullOutput {
    let events = if press_next {
        [true, false]
            .into_iter()
            .map(|pressed| egui::Event::Key {
                key: egui::Key::ArrowRight,
                physical_key: Some(egui::Key::ArrowRight),
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            })
            .collect()
    } else {
        Vec::new()
    };
    crate::test_ui::headless_paint_output(ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 400.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            ctx.root_ui(|root| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(root, |ui| run(ui, ctx));
            });
        },
    ))
}

fn draw(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) -> SubmittedUnitReceipt {
    let idx = app.fullscreen_idx.unwrap();
    let decision = app.fs_page_turn_decision_for_frame(ctx, idx);
    assert!(
        app.fs_nav_holdover_for_draw().is_none(),
        "paint the actual ready target"
    );
    let composition = app.spread_display_composition_for_anchor(idx);
    let screen = composition.pages_in_screen_order(app.spread_mode);
    let texture = app.resolve_fs_processed_material(ctx, idx, false);
    let resource = app.fullscreen_paint_payload_for_page(idx, texture, false);
    if idx == 1 && app.page_turn_presentation_pending() {
        assert!(
            resource.is_some(),
            "the accepted partial source must be a primary held resource, not a decode-wait thumbnail fallback"
        );
    }
    let thumbnail = app.fs_thumbnail_resource_for_display(idx).ok();
    let content_bbox = app.view_trim_single_content_bbox(idx);
    let mut sources = FsNavigatorTextureSources::default();
    app.fullscreen_page_layout
        .begin(FullscreenPageLayoutKind::Single);
    let rect = ui.max_rect();
    let transform = app
        .draw_fs_image_with_receipt(
            ui,
            rect,
            idx,
            app.fs_page_coordinate_source_size(idx),
            FsPageLayoutSource::CurrentItem,
            None,
            None,
            composition.singleton_placement(),
            resource.as_ref(),
            thumbnail.as_ref(),
            false,
            false,
            false,
            crate::rotation_db::Rotation::None,
            None,
            0.0,
            &FsBgStyle::Solid(egui::Color32::WHITE),
            "",
            false,
            FullscreenFitMode::Page,
            FullscreenFitScaleLimits::default(),
            content_bbox,
            Some(&mut sources),
            true,
            None,
        )
        .expect("a partial or underlay-only page still has a submitted transform");
    app.fullscreen_page_layout
        .push_occurrence(transform, screen[0]);
    assert!(sources.rebind_occurrences(&screen));
    let FsDisplayUnitSubmission::Paged(receipt) = &sources.submission else {
        panic!("ordinary draw must produce the page-turn receipt");
    };
    let receipt = receipt.clone();
    assert_eq!(receipt.pages().len(), 1);
    assert!(
        receipt.pages()[0].3 > 0.0,
        "page underlay contributes visible area even without image pixels"
    );
    app.emit_fs_page_turn_ready_for_display_unit(ctx, &composition, decision, &sources);
    receipt
}

fn next_key(app: &mut App, ctx: &egui::Context) -> ViewerOperationId {
    let operation = crate::key_input::operation_key_edges(ctx, egui::ViewportId::ROOT)
        .into_iter()
        .find(|edge| edge.pressed())
        .map(|edge| ViewerOperationId::Key(edge.operation_id()))
        .expect("real egui key ingress owns the navigation");
    assert!(app.handle_fullscreen_root_key_input(ctx));
    operation
}

fn run_coverage_lifecycle(empty: bool) {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    take_test_events();
    let origin = Instant::now();
    let clock = super::clock_tests::TestClock::at(origin);
    let ctx = egui::Context::default();
    install(&ctx);
    crate::key_input::install_operation_input_plugin(&ctx);
    ctx.data_mut(|data| data.insert_temp(egui::Id::unique("page_turn_input_enabled"), true));
    let mut app = ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 200;
    #[cfg(windows)]
    {
        app.native_video_in_window_active = false;
    }
    app.thumbnails = (0..app.items.len())
        .map(|_| ThumbnailState::Pending)
        .collect();
    let crop_width = if empty { 8 } else { 40 };
    let coverage = SourceCoverage::Known(
        EditPreviewGeometry::new(
            crate::page_dims::CanonicalPageDimensions::new(80, 120),
            (80, 120),
            (if empty { 0 } else { 20 }, 10, crop_width, 100),
        )
        .unwrap(),
    );
    if empty {
        app.view_trim_apply_mode = crate::view_trim::ViewTrimApplyMode::Book;
        app.view_trim_page_overrides.insert(
            1,
            crate::view_trim::ViewTrimPageOverride::from_margins(
                crate::view_trim::ViewTrimMargins {
                    left: 0.2,
                    ..Default::default()
                },
            ),
        );
        assert_eq!(app.view_trim_single_content_bbox(1).unwrap().min.x, 0.2);
    }
    // A decode-wait thumbnail fallback is intentionally not an animation's live outgoing unit.
    // Start at the ordinary source page 0, then turn into partial page 1 and back out to source
    // page 2, exercising both incoming and outgoing roles of the accepted partial material.
    for idx in [1] {
        app.fs_cache.remove(&idx);
        let pixels = Arc::new(egui::ColorImage::filled(
            [crop_width as usize / 2, 50],
            egui::Color32::LIGHT_BLUE,
        ));
        let tex = ctx.load_texture(
            format!("coverage-lifecycle-{idx}"),
            (*pixels).clone(),
            egui::TextureOptions::LINEAR,
        );
        app.thumbnails[idx] = ThumbnailState::loaded_raster(
            tex,
            crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
            true,
            50,
            coverage,
            Some((crop_width, 100)),
            None,
        );
        app.thumb_pixels.insert(idx, pixels);
        app.export_crop_pages.insert(idx);
    }

    let mut initial = None;
    frame(&ctx, false, |ui, ctx| {
        initial = Some(draw(&mut app, ui, ctx))
    });
    assert!(initial.unwrap().was_accepted());
    assert!(matches!(
        app.page_turn_presentation.state,
        PresentationState::Flat(_)
    ));

    let mut operation = None;
    let mut curl = None;
    let output = frame(&ctx, true, |ui, ctx| {
        operation = Some(next_key(&mut app, ctx));
        assert_eq!(app.fullscreen_idx, Some(1));
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Offered { .. }
        ));
        let receipt = draw(&mut app, ui, ctx);
        assert!(
            receipt.curl,
            "partial materials must take the ordinary animated path"
        );
        assert!(
            !receipt.was_accepted(),
            "acceptance belongs to the output hook"
        );
        curl = Some(receipt);
    });
    assert!(!output.shapes.is_empty());
    assert!(curl.unwrap().was_accepted());
    let tracker = match &app.page_turn_presentation.state {
        PresentationState::Turning {
            incoming, offer, ..
        } => {
            assert_eq!(Some(offer.fact.operation_id), operation);
            assert_eq!(incoming.pages[0].resource.source_coverage(), coverage);
            let canonical = canonical_unit(incoming).unwrap();
            if empty {
                assert!(matches!(
                    canonical.pages[0].material,
                    crate::page_turn_animation::PageMaterial::Blank { .. }
                ));
            } else {
                assert!(matches!(
                    canonical.pages[0].material,
                    crate::page_turn_animation::PageMaterial::PartialTexture { .. }
                ));
            }
            offer.submission.clone()
        }
        _ => panic!(
            "ordinary submitted output must enter Turning: {:?}",
            take_test_events()
        ),
    };
    assert_eq!(tracker.status(), SubmissionStatus::Started);
    clock.set(tracker.started_at().unwrap() + Duration::from_secs(1));
    let mut flat = None;
    frame(&ctx, false, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        let receipt = draw(&mut app, ui, ctx);
        assert!(!receipt.curl);
        assert!(!receipt.was_accepted());
        assert!(
            app.pending_fs_page_turn_ready.is_some(),
            "normal rendering queues the unaccepted ready notification"
        );
        assert!(
            app.page_turn_barrier_active(),
            "drawing alone cannot release the presentation wait"
        );
        assert!(!app.commit_page_turn_continuation(ctx));
        flat = Some(receipt);
    });
    assert!(flat.unwrap().was_accepted());
    assert_eq!(tracker.status(), SubmissionStatus::Finished);

    // Only the following owning frame consumes the accepted ready notification. The next real
    // key must then leave the old target and confirm page 2 without a stuck presentation wait.
    let mut next_curl = None;
    frame(&ctx, true, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        assert!(!app.page_turn_barrier_active());
        assert!(!app.fs_navigation_sequence_blocks_new_target());
        assert!(app.pending_fs_page_turn_ready.is_none());
        next_key(&mut app, ctx);
        assert_eq!(app.fullscreen_idx, Some(2));
        let receipt = draw(&mut app, ui, ctx);
        assert!(receipt.curl);
        next_curl = Some(receipt);
    });
    assert!(next_curl.unwrap().was_accepted());
    clock.set(origin + Duration::from_secs(2));
    frame(&ctx, false, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        assert!(!draw(&mut app, ui, ctx).curl);
    });
    frame(&ctx, false, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        draw(&mut app, ui, ctx);
        assert_eq!(app.fullscreen_idx, Some(2));
        assert!(!app.page_turn_barrier_active());
        assert!(!app.fs_navigation_sequence_blocks_new_target());
    });
    let events = take_test_events();
    let events: Vec<_> = events
        .iter()
        .filter(|(fact, _, _)| Some(fact.operation_id) == operation)
        .collect();
    for expected in ["opportunity", "offered", "start", "finish", "fully_visible"] {
        assert_eq!(
            events
                .iter()
                .filter(|(_, kind, _)| *kind == expected)
                .count(),
            1,
            "{expected}: {events:?}"
        );
    }
    assert!(
        !events
            .iter()
            .any(|(_, kind, _)| matches!(*kind, "skip" | "cancel")),
        "{events:?}"
    );
}

#[test]
fn crop_coverage_partial_material_real_handler_completes_and_accepts_next_page() {
    run_coverage_lifecycle(false);
}

#[test]
fn crop_coverage_empty_intersection_real_handler_completes_and_accepts_next_page() {
    run_coverage_lifecycle(true);
}
