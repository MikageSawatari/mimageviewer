use super::{phase_c_support::setup_app, *};
use egui_kittest::kittest::Queryable;

fn request_whats_new(app: &mut App) {
    app.settings.whats_new_acknowledged_version = Some("3.9.0".to_owned());
    app.whats_new_entries = crate::version_highlights::highlights_to_show(
        app.settings.whats_new_acknowledged_version.as_deref(),
        env!("CARGO_PKG_VERSION"),
        crate::version_highlights::table(),
    );
    assert!(!app.whats_new_entries.is_empty());
    app.show_whats_new = true;
    app.show_mouse_nav_migration_prompt = false;
    app.network_data_dir_notice = None;
}

fn escape_input() -> egui::RawInput {
    egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    }
}

fn deferred_navigation_reopen() -> DeferredFsReopen {
    DeferredFsReopen {
        history_trigger: HistoryTrigger::UserChosen,
        resume_slideshow: false,
        target: DeferredFsTarget::None,
        resume_to_last_page: false,
        from_explicit_open: false,
        preserve_after_password_prompt: false,
    }
}

#[cfg(windows)]
pub(in crate::app) fn close_waiting_viewer_through_backend(
    app: &mut App,
    ctx: &egui::Context,
    detached: bool,
    mounted: bool,
) {
    app.fs_viewport_shown = true;
    app.fs_viewport_presentation = Some(if detached {
        ViewerPresentation::DetachedWindow
    } else {
        ViewerPresentation::Fullscreen
    });
    let viewport_id = if detached {
        app.viewer_presentation = ViewerPresentation::DetachedWindow;
        app.stash_mounted_as_active_for_test(1236);
        // Install the real runtime/lease as well as the at-rest bundle binding.
        app.begin_active_detached_session(1236, DetachedSource::Image);
        App::detached_image_window_viewport_id(1236)
    } else {
        app.viewer_presentation = ViewerPresentation::Fullscreen;
        app.fullscreen_viewport_id()
    };
    ctx.set_embed_viewports(false);
    ctx.run_ui(Default::default(), |ui| {
        ctx.show_viewport_deferred(viewport_id, egui::ViewportBuilder::default(), |_, _| {});
        if detached {
            app.capture_detached_viewport_registration(ui.ctx(), 1236);
        } else {
            app.fullscreen_viewport_binding =
                viewport_retirement::ViewportCommandBinding::capture(ui.ctx(), viewport_id);
        }
    })
    .drop_without_applying_deltas();
    assert!(ctx.viewport_registration(viewport_id).is_some());
    let accept = |app: &mut App| {
        assert!(matches!(
            eframe::App::on_viewport_close_requested(app, ctx, viewport_id),
            eframe::ViewportCloseResponse::RetirementTransferred
        ));
    };
    if mounted {
        app.with_active_viewer_context(accept)
            .expect("mounted detached close");
    } else {
        accept(app);
    }
}

fn assert_startup_notices_deferred(app: &mut App) {
    assert!(!app.whats_new_visible());
    assert!(!app.common_modal_dialog_open());
    assert!(!app.any_modal_dialog_open_for_fullscreen_keys());
    let ctx = egui::Context::default();
    ctx.begin_pass(escape_input());
    app.show_whats_new_dialog(&ctx);
    assert!(app.dialog_escape_pressed(&ctx));
    let output = ctx.end_pass();
    assert!(output.shapes.is_empty());
    output.drop_without_applying_deltas();
    assert!(app.show_whats_new);
    assert_eq!(
        app.settings.whats_new_acknowledged_version.as_deref(),
        Some("3.9.0")
    );

    // The same owner also defers the following network notice and the other
    // main-only startup prompts, even if What's New is not requested.
    app.show_whats_new = false;
    app.network_data_dir_notice = Some(PathBuf::from(r"\\server\share\data"));
    assert!(!app.network_data_dir_notice_visible());
    assert!(!app.common_modal_dialog_open());
    app.network_data_dir_notice = None;
    app.show_mouse_nav_migration_prompt = true;
    assert!(!app.mouse_nav_migration_prompt_visible());
    assert!(!app.common_modal_dialog_open());
    app.show_mouse_nav_migration_prompt = false;
    app.settings.first_setup_completed = false;
    assert!(!app.first_setup_visible());
    assert!(!app.common_modal_dialog_open());
    app.settings.first_setup_completed = true;
    app.show_whats_new = true;
}

#[test]
fn startup_notices_defer_through_enumeration_reopen_and_show_after_cancel() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    app.items = vec![GridItem::Image(PathBuf::from("notice-test.jpg"))];
    for presentation in [
        ViewerPresentation::Fullscreen,
        ViewerPresentation::MainWindow,
        ViewerPresentation::DetachedWindow,
    ] {
        app.viewer_presentation = presentation;
        // Both PDF and ZIP navigation transfer the reopen to this owner while
        // the index is absent. Embedded mode retains its main surface instead
        // of an OS viewport, so fs_viewport_shown is deliberately false there.
        app.fs_viewport_shown = presentation != ViewerPresentation::MainWindow;
        #[cfg(windows)]
        {
            app.native_video_in_window_active = presentation == ViewerPresentation::MainWindow;
        }
        app.fullscreen_idx = None;
        app.fs_nav_after_pdf_enumerate = Some(deferred_navigation_reopen());
        assert_startup_notices_deferred(&mut app);

        // Enumeration completes and the viewer adopts a page: the notice must
        // stay deferred after the old pending owner is consumed.
        app.fs_nav_after_pdf_enumerate = None;
        app.fullscreen_idx = Some(0);
        assert_startup_notices_deferred(&mut app);

        // Another navigation is cancelled by the real waiting-surface close
        // boundary. Viewport cleanup may still be pending; it is not a viewer.
        app.fullscreen_idx = None;
        app.fs_nav_after_pdf_enumerate = Some(deferred_navigation_reopen());
        app.finish_fullscreen_navigation_for_true_close();
        assert!(app.fs_nav_after_pdf_enumerate.is_none());
        assert!(app.whats_new_visible(), "{presentation:?}");
        assert_eq!(app.modal_dialog_block_reason(), Some("whats_new"));
    }
}

#[cfg(windows)]
#[test]
fn startup_notices_defer_during_mounted_detached_preparation_without_an_index() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    app.build_active_context_for_test(Some(1234), DetachedSource::Image, |owner| {
        owner.fullscreen_idx = None;
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
    });
    assert!(!app.whats_new_visible());
    app.with_active_viewer_context(|owner| {
        assert!(!owner.active_detached_context_is_at_rest());
        assert!(owner.detached_active_window_alive_wanted());
        assert!(owner.active_detached_transition_outstanding());
        assert!(!owner.fs_nav_deferred_reopen_wait_active());
        assert_startup_notices_deferred(owner);
    })
    .expect("preparing detached context must mount");
    app.remove_active_context_for_test();
    assert!(app.whats_new_visible());
}

#[cfg(windows)]
#[test]
fn startup_notices_archive_wait_escape_and_close_keep_work_but_retire_late_reopen() {
    use crate::ui_dialogs::archive_convert::{
        ArchiveConvertCompletionPolicy, ArchiveConvertMsg, ArchiveConvertPhase,
        ArchiveConvertProgressShared, ArchiveConvertState, ArchiveScanOutcome,
    };
    for direct in [false, true] {
        for close in [
            "escape",
            "ui_close",
            "backend_fullscreen",
            "backend_detached",
            "backend_detached_mounted",
            "context_retire",
            "retired_owner_at_completion",
            "internal_reopen_then_backend_fullscreen",
        ] {
            let escape = close == "escape";
            let mut app = setup_app();
            request_whats_new(&mut app);
            let source = app.tmp.path().join("source.7z");
            std::fs::write(&source, b"source").unwrap();
            let backing = app.tmp.path().join("readable.zip");
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&backing).unwrap());
            zip.start_file("page-001.jpg", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut zip, b"page").unwrap();
            zip.finish().unwrap();
            let cancel = Arc::new(AtomicBool::new(false));
            let (deliver, rx) = mpsc::channel();
            app.archive_convert = Some(Box::new(ArchiveConvertState {
                src_path: source.clone(),
                input_seq: 1232,
                format: if direct {
                    ArchiveFormat::Rar
                } else {
                    ArchiveFormat::SevenZ
                },
                password: None,
                password_input: String::new(),
                phase: if direct {
                    ArchiveConvertPhase::Scanning
                } else {
                    ArchiveConvertPhase::Converting {
                        progress: Arc::new(ArchiveConvertProgressShared::new()),
                    }
                },
                cancel: Arc::clone(&cancel),
                rx,
                pending_nav: None,
                pending_direct_nav: None,
                allow_direct_read: direct,
                completion: ArchiveConvertCompletionPolicy::Navigation,
                restore_intent: StartupListIntent::PageContinuation,
                pending_sibling_output: None,
                auto_fullscreen: false,
                deferred_fullscreen: None,
                suppress_confirm: true,
                suppress_confirm_next_time: false,
            }));
            app.settings.auto_fullscreen_zip_pdf = true;
            app.fs_nav_locked_gen = Some(app.items_generation);
            app.fs_holdover_tex = Some(FsHoldover::FolderNavigation(None));
            assert!(app.attach_archive_convert_deferred_fullscreen(
                false,
                false,
                HistoryTrigger::UserChosen
            ));
            let original_reopen = app
                .archive_convert
                .as_ref()
                .unwrap()
                .deferred_fullscreen
                .clone();
            if direct {
                // Suppressed scanning is not modal. A sibling viewer may close while
                // main's archive request is still pending; that is not its cancellation.
                app.build_active_context_for_test(Some(1235), DetachedSource::Image, |owner| {
                    owner.items = vec![GridItem::Image(PathBuf::from("sibling.jpg"))];
                    owner.fullscreen_idx = Some(0);
                    owner.viewer_presentation = ViewerPresentation::DetachedWindow;
                });
                let sibling_ctx = egui::Context::default();
                sibling_ctx.begin_pass(escape_input());
                app.with_active_viewer_context(|owner| {
                    let action = owner.handle_fs_key_input(&sibling_ctx, 0, false);
                    assert!(action.close, "sibling Escape must be accepted");
                    owner.handle_fullscreen_close_request();
                })
                .expect("sibling viewer must mount");
                sibling_ctx.end_pass().drop_without_applying_deltas();
                app.remove_active_context_for_test();
                assert!(
                    app.archive_convert_deferred_fullscreen_active(),
                    "sibling close must preserve main's reopen"
                );
                assert!(!cancel.load(Ordering::Relaxed));
            }
            app.viewer_presentation = ViewerPresentation::MainWindow;
            app.native_video_in_window_active = true;
            assert!(!app.whats_new_visible());
            let mut input = if escape {
                escape_input()
            } else {
                egui::RawInput::default()
            };
            if !escape {
                input
                    .viewports
                    .get_mut(&egui::ViewportId::ROOT)
                    .unwrap()
                    .events
                    .push(egui::ViewportEvent::Close);
            }
            let ctx = egui::Context::default();
            if close == "internal_reopen_then_backend_fullscreen" {
                let owner = app.projected_viewer_context_id();
                app.close_fullscreen_for_folder_nav_reopen();
                assert!(app.archive_convert_deferred_fullscreen_active());
                assert_eq!(
                    app.archive_convert
                        .as_ref()
                        .unwrap()
                        .deferred_fullscreen
                        .as_ref()
                        .unwrap()
                        .owner_context,
                    owner
                );
                assert!(!app.whats_new_visible());
            }
            if close.starts_with("backend_") || close.starts_with("internal_reopen_") {
                close_waiting_viewer_through_backend(
                    &mut app,
                    &ctx,
                    close.starts_with("backend_detached"),
                    close == "backend_detached_mounted",
                );
            } else if matches!(close, "context_retire" | "retired_owner_at_completion") {
                let retired = app.stash_mounted_as_active_for_test(1236);
                // Retirement must not clear the currently mounted sibling's holdover.
                app.fs_nav_locked_gen = Some(app.items_generation);
                app.fs_holdover_tex = Some(FsHoldover::FolderNavigation(None));
                app.retire_context(retired, "notice_test_source_retired", |_| ())
                    .unwrap();
                assert!(app.fs_nav_is_locked());
                assert!(app.fs_holdover_tex.is_some());
                app.release_fs_nav_lock();
            } else {
                ctx.run_pass(input, |ctx| {
                    ctx.root_ui(|ui| app.render_fullscreen_viewport(ui));
                    assert!(!app.archive_convert_deferred_fullscreen_active());
                    assert!(app.whats_new_visible());
                    if escape {
                        assert!(!app.dialog_escape_pressed(ctx));
                        app.show_whats_new_dialog(ctx);
                        assert!(
                            app.show_whats_new,
                            "viewer Escape must not acknowledge the notice"
                        );
                    }
                })
                .drop_without_applying_deltas();
            }
            assert!(
                !app.archive_convert_deferred_fullscreen_active(),
                "{direct} {close}"
            );
            assert!(app.whats_new_visible(), "{direct} {close}");
            assert!(
                !cancel.load(Ordering::Relaxed),
                "closing the viewer must keep conversion running"
            );
            assert!(app.archive_convert.is_some());
            assert!(app.fs_holdover_tex.is_none());
            assert!(!app.fs_nav_is_locked());

            if close == "retired_owner_at_completion" {
                // Exercise completion's identity check independently of eager retirement:
                // even a stale reservation carrying the old identity cannot authorize open.
                app.archive_convert.as_mut().unwrap().deferred_fullscreen = original_reopen;
            }

            // Deliver the original lifecycle's success only AFTER the real close.
            // Its list adoption remains useful, but it no longer owns a viewer open.
            let message = if direct {
                ArchiveConvertMsg::ScanDone(Ok(ArchiveScanOutcome::Direct {
                    source: backing.clone(),
                }))
            } else {
                ArchiveConvertMsg::ConvertDone(Ok((
                    crate::archive_converter::ArchiveImageSummary {
                        image_count: 1,
                        total_uncompressed_bytes: 4,
                        nested_archive_count: 0,
                    },
                    backing.clone(),
                    std::fs::metadata(&backing).unwrap().len() as i64,
                )))
            };
            deliver.send(message).unwrap();
            ctx.run_pass(egui::RawInput::default(), |ctx| {
                app.show_archive_convert_dialog(ctx)
            })
            .drop_without_applying_deltas();
            assert!(app.archive_convert.is_none());
            // Cached conversion may classify the path and use prepared history adoption;
            // direct reads enter the ordinary ZIP receiver. Drive both existing owners.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                app.poll_rar_archive_navigation_for_test(&ctx);
                app.poll_zip_enumerate();
                app.poll_sidecar_restore(&ctx);
                if app.current_folder.as_ref() == Some(&backing)
                    && app.zip_enumerate_pending.is_none()
                    && app.top_level_grid_view.open_path_classification().is_none()
                    && app
                        .top_level_grid_view
                        .history_navigation_transition()
                        .is_none()
                    && !app.sidecar_restore_active()
                {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "late archive list adoption did not settle"
                );
                std::thread::yield_now();
            }
            assert!(
                app.items
                    .iter()
                    .any(|item| matches!(item, GridItem::ZipImage { .. }))
            );
            assert_eq!(app.current_folder.as_ref(), Some(&backing));
            assert!(app.fullscreen_idx.is_none());
            assert!(app.fs_nav_after_pdf_enumerate.is_none());
            assert!(!app.pending_auto_fs_open);
            assert!(app.whats_new_visible());
            assert_eq!(
                app.settings.whats_new_acknowledged_version.as_deref(),
                Some("3.9.0")
            );
        }
    }
}

#[test]
fn startup_notices_defer_drawing_modal_and_escape_in_every_viewer_mode() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    app.items = vec![GridItem::Image(PathBuf::from("notice-test.jpg"))];
    for presentation in [
        ViewerPresentation::Fullscreen,
        ViewerPresentation::MainWindow,
        ViewerPresentation::DetachedWindow,
    ] {
        app.viewer_presentation = presentation;
        app.fullscreen_idx = Some(0);
        assert!(!app.whats_new_visible(), "{presentation:?}");
        assert!(!app.common_modal_dialog_open(), "{presentation:?}");
        assert!(!app.any_modal_dialog_open_for_fullscreen_keys());
        let ctx = egui::Context::default();
        ctx.begin_pass(escape_input());
        app.show_whats_new_dialog(&ctx);
        assert!(
            app.dialog_escape_pressed(&ctx),
            "pending notice must not consume Escape"
        );
        let output = ctx.end_pass();
        assert!(output.shapes.is_empty(), "pending notice must not draw");
        output.drop_without_applying_deltas();
        assert!(app.show_whats_new);
        assert_eq!(
            app.settings.whats_new_acknowledged_version.as_deref(),
            Some("3.9.0")
        );

        // Closing the viewer exposes the request; reopening hides it again without a latch.
        app.fullscreen_idx = None;
        assert!(app.whats_new_visible());
        assert_eq!(app.modal_dialog_block_reason(), Some("whats_new"));
    }
}

#[test]
fn startup_notices_embedded_viewer_escape_does_not_acknowledge_notice_in_same_pass() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    app.items = vec![GridItem::Image(PathBuf::from("notice-test.jpg"))];
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::MainWindow;
    let ctx = egui::Context::default();
    ctx.begin_pass(escape_input());
    // The deferred dialog does not inspect Escape. The real viewer handler consumes it.
    app.show_whats_new_dialog(&ctx);
    let action = app.handle_fs_key_input(&ctx, 0, false);
    assert!(action.close, "viewer must accept Escape");
    app.handle_fullscreen_close_request();
    assert!(
        app.fullscreen_idx.is_none(),
        "viewer Escape must remain usable"
    );
    app.show_whats_new_dialog(&ctx);
    assert!(
        app.show_whats_new,
        "the same Escape must not dismiss the newly visible notice"
    );
    assert_eq!(
        app.settings.whats_new_acknowledged_version.as_deref(),
        Some("3.9.0")
    );
    ctx.end_pass().drop_without_applying_deltas();
}

#[cfg(windows)]
#[test]
fn startup_notices_wait_for_active_and_other_detached_viewers() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    let ctx = egui::Context::default();
    app.build_active_context_for_test(Some(1232), DetachedSource::Image, |owner| {
        owner.fullscreen_idx = Some(0);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
    });
    assert!(app.fullscreen_idx.is_none());
    assert!(app.active_detached_context_is_at_rest());
    assert!(!app.whats_new_visible());
    assert!(!app.common_modal_dialog_open());
    app.remove_active_context_for_test();
    assert!(app.whats_new_visible());

    app.push_window_context_for_test(&ctx, 1233, |owner| {
        owner.fullscreen_idx = Some(0);
        owner.viewer_presentation = ViewerPresentation::DetachedWindow;
    });
    assert!(app.fullscreen_idx.is_none());
    assert!(!app.whats_new_visible());
    assert!(!app.common_modal_dialog_open());
}

#[test]
fn startup_notices_network_notice_waits_for_viewer_and_prior_notices() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    app.network_data_dir_notice = Some(PathBuf::from(r"\\server\share\data"));
    for presentation in [
        ViewerPresentation::Fullscreen,
        ViewerPresentation::MainWindow,
        ViewerPresentation::DetachedWindow,
    ] {
        app.viewer_presentation = presentation;
        app.fullscreen_idx = Some(0);
        assert!(!app.network_data_dir_notice_visible());
        app.show_whats_new = false;
        assert!(!app.network_data_dir_notice_visible());
        let ctx = egui::Context::default();
        ctx.begin_pass(escape_input());
        app.show_network_data_dir_notice_dialog(&ctx);
        let output = ctx.end_pass();
        assert!(output.shapes.is_empty());
        output.drop_without_applying_deltas();
        assert!(app.network_data_dir_notice.is_some());
        app.fullscreen_idx = None;
        app.show_whats_new = true;
        assert!(!app.network_data_dir_notice_visible());
        app.show_whats_new = false;
        app.show_mouse_nav_migration_prompt = true;
        assert!(!app.network_data_dir_notice_visible());
        app.show_mouse_nav_migration_prompt = false;
        assert!(app.network_data_dir_notice_visible());
        assert_eq!(
            app.modal_dialog_block_reason(),
            Some("network_data_dir_notice")
        );
    }
}

#[test]
fn startup_notices_whats_new_escape_does_not_dismiss_following_network_notice() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    app.network_data_dir_notice = Some(PathBuf::from(r"\\server\share\data"));
    let ctx = egui::Context::default();
    let mut input = escape_input();
    // A modifier may have been released later in the same input batch. Consume the
    // actual Escape events rather than matching their modifier against current level.
    if let egui::Event::Key { modifiers, .. } = &mut input.events[0] {
        *modifiers = egui::Modifiers::CTRL;
    }
    ctx.begin_pass(input);
    app.show_whats_new_dialog(&ctx);
    assert!(!app.show_whats_new);
    assert!(app.network_data_dir_notice_visible());
    app.show_network_data_dir_notice_dialog(&ctx);
    assert!(app.network_data_dir_notice.is_some());
    assert_eq!(
        app.modal_dialog_block_reason(),
        Some("network_data_dir_notice")
    );
    ctx.end_pass().drop_without_applying_deltas();
}

#[test]
fn startup_notices_help_preserves_pending_cumulative_entries_and_reopens_latest_after_dismissal() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    let expected: Vec<_> = app
        .whats_new_entries
        .iter()
        .map(|entry| entry.version)
        .collect();
    assert!(expected.len() > 1);
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.request_whats_new_from_help();
    assert_eq!(
        app.whats_new_entries
            .iter()
            .map(|entry| entry.version)
            .collect::<Vec<_>>(),
        expected
    );
    assert!(!app.whats_new_visible());
    app.fullscreen_idx = None;
    app.show_whats_new = false;
    app.whats_new_entries.clear();
    app.request_whats_new_from_help();
    assert!(app.whats_new_visible());
    let latest = crate::version_highlights::latest_not_newer_than(
        env!("CARGO_PKG_VERSION"),
        crate::version_highlights::table(),
    )
    .unwrap();
    assert_eq!(app.whats_new_entries.len(), 1);
    assert_eq!(app.whats_new_entries[0].version, latest.version);
}

#[test]
fn startup_notices_acknowledgement_save_failure_notifies_without_removing_stored_unread_version() {
    let mut app = setup_app();
    request_whats_new(&mut app);
    assert!(app.settings.save_checked());
    crate::settings_db::set_save_suppressed(true);
    let ctx = egui::Context::default();
    ctx.begin_pass(escape_input());
    app.show_whats_new_dialog(&ctx);
    assert!(!app.show_whats_new);
    assert!(
        app.fs_feedback_toast
            .as_ref()
            .unwrap()
            .0
            .contains("保存できませんでした")
    );
    assert_eq!(
        app.fs_feedback_toast_surface,
        Some(ActionSurface::MainWindow)
    );
    ctx.end_pass().drop_without_applying_deltas();
    crate::settings_db::set_save_suppressed(false);
    let stored = crate::settings_db::with_db_result(|db| db.load_into_settings()).unwrap();
    assert_eq!(
        stored.whats_new_acknowledged_version.as_deref(),
        Some("3.9.0")
    );
}

#[test]
fn startup_notices_constructor_uses_acknowledgement_instead_of_startup_version() {
    let settings = crate::settings::Settings {
        last_seen_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        whats_new_acknowledged_version: Some("3.9.0".to_owned()),
        ..Default::default()
    };
    let app = super::phase_c_support::setup_paused_similar_app_with_fixture(settings, |_| {});
    assert!(app.show_whats_new);
    let expected = crate::version_highlights::highlights_to_show(
        Some("3.9.0"),
        env!("CARGO_PKG_VERSION"),
        crate::version_highlights::table(),
    );
    assert_eq!(
        app.whats_new_entries
            .iter()
            .map(|e| e.version)
            .collect::<Vec<_>>(),
        expected.iter().map(|e| e.version).collect::<Vec<_>>()
    );
}

#[test]
fn startup_notices_first_setup_and_mouse_migration_share_visible_modal_policy() {
    let mut app = setup_app();
    app.settings.first_setup_completed = false;
    app.show_mouse_nav_migration_prompt = true;
    for presentation in [
        ViewerPresentation::Fullscreen,
        ViewerPresentation::MainWindow,
        ViewerPresentation::DetachedWindow,
    ] {
        app.viewer_presentation = presentation;
        app.fullscreen_idx = Some(0);
        assert!(!app.first_setup_visible());
        assert!(!app.mouse_nav_migration_prompt_visible());
        assert!(!app.common_modal_dialog_open());
        let ctx = egui::Context::default();
        ctx.begin_pass(escape_input());
        app.show_first_setup_dialog(&ctx);
        app.show_mouse_nav_migration_prompt_dialog(&ctx);
        let output = ctx.end_pass();
        assert!(output.shapes.is_empty());
        output.drop_without_applying_deltas();
        assert!(!app.settings.first_setup_completed);
        assert!(app.show_mouse_nav_migration_prompt);
        app.fullscreen_idx = None;
        assert!(app.first_setup_visible());
        assert!(app.mouse_nav_migration_prompt_visible());
        assert!(app.common_modal_dialog_open());
    }
}

#[test]
fn startup_notices_dismissal_saves_current_version_for_all_close_actions() {
    for action in ["閉じる", "Close window", "Escape"] {
        let mut app = setup_app();
        request_whats_new(&mut app);
        let mut fonts_ready = false;
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .build_ui_state(
                move |ui, app: &mut super::phase_c_support::AppTestEnv| {
                    if !fonts_ready {
                        crate::ui_fonts::configure_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    app.show_whats_new_dialog(ui.ctx());
                },
                app,
            );
        harness.run_steps(5);
        harness.get_by_label("閉じる");
        if action == "Escape" {
            harness.event(escape_input().events.remove(0));
        } else {
            harness.get_by_label(action).click_accesskit();
        }
        harness.run_steps(2);
        assert!(!harness.state().show_whats_new, "{action}");
        let stored = crate::settings_db::with_db_result(|db| db.load_into_settings()).unwrap();
        assert_eq!(
            stored.whats_new_acknowledged_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION")),
            "{action}"
        );
        assert!(
            crate::version_highlights::highlights_to_show(
                stored.whats_new_acknowledged_version.as_deref(),
                env!("CARGO_PKG_VERSION"),
                crate::version_highlights::table(),
            )
            .is_empty()
        );
    }
}
