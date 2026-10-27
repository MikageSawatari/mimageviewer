//! Overview admission preserves the existing adopted-flat navigation boundary.
use super::page_turn_submission_tests::{draw_current, frame, ready_app, seed_leaf};
use super::*;

fn overview_app(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    crate::key_input::install_operation_input_plugin(ctx);
    let mut app = ready_app(ctx, SpreadMode::Single);
    app.settings.video_in_window_mode = true;
    app.viewer_presentation = ViewerPresentation::MainWindow;
    app.native_video_in_window_active = true;
    app.page_identity_index = Arc::new(
        crate::app::file_type_resume::PreparedPageIdentityIndex::prepare(
            &app.items,
            None,
            app.settings.file_type_visibility.clone(),
        ),
    );
    for idx in 0..4 {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
    }
    seed_leaf(ctx, &mut app);
    app
}

fn key_frame(
    ctx: &egui::Context,
    key: egui::Key,
    mut run: impl FnMut(&mut egui::Ui, &egui::Context),
) {
    let events = [true, false]
        .map(|pressed| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        })
        .to_vec();
    let _ = crate::test_ui::headless_paint_output(ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 400.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            ctx.root_ui(|root_ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(root_ui, |ui| run(ui, ctx));
            });
        },
    ));
}

#[test]
fn overview_o_during_curl_waits_for_normal_flat_before_fresh_o_and_confirm() {
    assert_overview_admission_after_flat(false);
}

#[test]
fn overview_o_preserves_accepted_navigation_until_flat_and_continuation_land() {
    assert_overview_admission_after_flat(true);
}

fn assert_overview_admission_after_flat(with_continuation: bool) {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = overview_app(&ctx);
    assert!(app.page_turn_barrier_active(), "real accepted curl fixture");
    assert!(!app.page_turn_completion_active());
    // The shared entry point also protects non-key entrances without cancelling
    // the turn or changing the already accepted target.
    frame(&ctx, |_, _| {
        assert!(!app.open_page_overview(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0)),
            Default::default(),
        ));
    });
    assert!(
        !app.page_turn_completion_active(),
        "refusal does not cancel"
    );
    assert_eq!(app.fullscreen_idx, Some(1));

    if with_continuation {
        frame(&ctx, |_, ctx| {
            let operation = ViewerOperationId::Slideshow(page_turn::next_operation_batch());
            app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "slideshow");
            app.handle_fs_navigation_with_operation(
                ctx,
                false,
                false,
                None,
                None,
                None,
                FsPageNav::Target(2),
                None,
                1,
                Some(operation),
            );
            assert_eq!(app.fullscreen_idx, Some(1));
            assert!(app.page_turn_completion_active());
        });
    }

    key_frame(&ctx, egui::Key::O, |ui, ctx| {
        assert!(matches!(
            app.begin_page_turn_viewer_frame(ctx, true, None),
            PageTurnFrameEntry::Continue {
                committed: false,
                ..
            }
        ));
        app.handle_fs_key_input(ctx, 1, false);
        assert!(app.page_overview.session().is_none(), "O is not queued");
        assert_eq!(app.fullscreen_idx, Some(1));
        assert!(app.page_turn_barrier_active());
        assert!(!app.open_page_overview_for_viewer(ctx));
        // Ordinary operation observation may cancel curl into its mandatory
        // flat. Only the normal page submission/output owner can complete it.
        let (composition, sources) = draw_current(&mut app, ui, ctx);
        let FsDisplayUnitSubmission::Paged(receipt) = &sources.submission else {
            panic!("paged flat receipt");
        };
        assert!(!receipt.curl);
        app.emit_fs_page_turn_ready_for_display_unit(
            ctx,
            &composition,
            FsPageTurnDecision::normal(),
            &sources,
        );
        assert!(
            !app.commit_page_turn_continuation(ctx),
            "same-frame flat cannot complete"
        );
        assert!(app.page_turn_barrier_active());
        assert!(!app.open_page_overview_for_viewer(ctx));
    });

    let landed = if with_continuation { 2 } else { 1 };
    frame(&ctx, |ui, ctx| {
        assert!(matches!(
            app.begin_page_turn_viewer_frame(ctx, true, None),
            PageTurnFrameEntry::Continue { .. }
        ));
        assert!(!app.page_turn_barrier_active());
        assert_eq!(app.fullscreen_idx, Some(landed), "accepted move survives O");
        app.handle_fs_key_input(ctx, landed, false);
        assert!(app.page_overview.session().is_none(), "no delayed entrance");
        let (composition, sources) = draw_current(&mut app, ui, ctx);
        app.emit_fs_page_turn_ready_for_display_unit(
            ctx,
            &composition,
            FsPageTurnDecision::normal(),
            &sources,
        );
    });

    key_frame(&ctx, egui::Key::O, |_, ctx| {
        app.begin_page_turn_viewer_frame(ctx, true, None);
        assert!(!app.fs_navigation_sequence_blocks_new_target());
        app.handle_fs_key_input(ctx, landed, false);
        assert!(
            app.page_overview.session().is_some(),
            "fresh O opens after completion"
        );
        assert!(!app.page_turn_barrier_active());
    });
    key_frame(&ctx, egui::Key::End, |_, ctx| {
        app.handle_fs_key_input(ctx, landed, false);
        assert_eq!(app.page_overview_view().unwrap().cursor, 3);
    });
    key_frame(&ctx, egui::Key::Enter, |_, ctx| {
        assert!(matches!(
            app.begin_page_turn_viewer_frame(ctx, true, None),
            PageTurnFrameEntry::Continue { .. }
        ));
        app.handle_fs_key_input(ctx, landed, false);
        assert!(
            app.page_overview.session().is_none(),
            "Enter confirms the overview"
        );
        assert_eq!(
            app.fullscreen_idx,
            Some(3),
            "Enter must not close the viewer"
        );
    });
}
