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
fn register_waiting_viewer(app: &mut App, ctx: &egui::Context, detached: bool) -> egui::ViewportId {
    app.native_video_in_window_active = false;
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
    viewport_id
}

#[cfg(windows)]
pub(in crate::app) fn close_waiting_viewer_through_backend(
    app: &mut App,
    ctx: &egui::Context,
    detached: bool,
    mounted: bool,
) {
    let viewport_id = register_waiting_viewer(app, ctx, detached);
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

/// The same real session-ending entry points are exercised with enumeration, book
/// projection / permission, and archive completion owners (not a test-only retire).
#[cfg(windows)]
pub(in crate::app) const WAITING_SESSION_END_ROUTES: &[&str] = &[
    "scale_fullscreen",
    "scale_embedded",
    "scale_detached",
    "bulk_mode_fullscreen",
    "bulk_mode_transition",
    "bulk_font_embedded",
    "direct_close",
    "page_list",
    "everything",
    "tray_close",
    "location_favorite",
    "ring_location",
];

#[cfg(windows)]
pub(in crate::app) fn end_waiting_viewer(app: &mut App, ctx: &egui::Context, close: &str) {
    if matches!(
        close,
        "scale_fullscreen" | "bulk_mode_fullscreen" | "bulk_mode_transition" | "scale_detached"
    ) {
        register_waiting_viewer(app, ctx, close == "scale_detached");
    } else {
        app.viewer_presentation = ViewerPresentation::MainWindow;
        app.native_video_in_window_active = true;
        app.fs_viewport_shown = false;
        app.fs_viewport_presentation = None;
    }
    match close {
        "scale_fullscreen" | "scale_embedded" | "scale_detached" => {
            app.set_ui_scale_factor(ctx, app.settings.ui_scale_factor + 0.25);
        }
        "bulk_mode_transition" => {
            begin_test_video_presentation_transition(
                app,
                ViewerPresentation::DetachedWindow,
                false,
            );
            assert!(app.video_presentation_transition.is_transitioning());
            app.close_all_detached_viewers_for_mode_change(ctx).unwrap();
        }
        "bulk_mode_fullscreen" => {
            app.close_all_detached_viewers_for_mode_change(ctx).unwrap();
        }
        "bulk_font_embedded" => {
            app.close_all_viewer_sessions_with_cause(ctx, ViewerCloseCause::Replaced)
                .unwrap();
        }
        "direct_close" => app.close_fullscreen(),
        "page_list" => app.close_fullscreen_to_page_list(),
        "everything" => app.finish_everything_edit_boundary(),
        "tray_close" => app.close_main_still_fullscreen_for_tray(),
        "location_favorite" => {
            app.settings.favorites = vec![crate::settings::FavoriteEntry::new(
                "notice destination".into(),
                PathBuf::from("notice-destination"),
            )];
            assert!(
                app.apply_location_navigation_key_action(
                    ctx,
                    KeyAction::GridOpenFavorite1,
                    "notice_test",
                )
                .is_some()
            );
        }
        "ring_location" => {
            assert!(
                app.apply_ring_action(
                    ctx,
                    crate::ring_shortcut::RingShortcutContext::ImageFullscreen,
                    crate::ring_shortcut::RingActionId::OpenLocationDriveList,
                    "notice_test",
                )
                .is_some()
            );
        }
        _ => panic!("unknown session end: {close}"),
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

#[cfg(windows)]
#[test]
fn startup_notices_settings_and_list_exits_retire_enumeration_before_late_success() {
    for pdf in [false, true] {
        for &close in WAITING_SESSION_END_ROUTES {
            let mut app = setup_app();
            request_whats_new(&mut app);
            let path = app
                .tmp
                .path()
                .join(if pdf { "late.pdf" } else { "late.zip" });
            let ctx = egui::Context::default();
            let mut pdf_complete = None;
            let mut zip_complete = None;
            let cancel = if pdf {
                let (handle, complete) =
                    crate::pdf_loader::pending_enumerate_handle_for_test(&path);
                let cancel = Arc::clone(&handle.cancel);
                pdf_complete = Some(complete);
                app.pdf_enumerate_pending = Some((
                    path.clone(),
                    None,
                    handle,
                    Box::new(OpenRequestOwner::Navigation),
                    None,
                    PdfOpenPhase::ColdCandidate {
                        retained_source: None,
                        restore_intent: StartupListIntent::PageContinuation,
                    },
                    crate::pdf_loader::LeasedEpubPath::try_new(path.clone()).unwrap(),
                ));
                cancel
            } else {
                let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
                zip.start_file("page.jpg", zip::write::SimpleFileOptions::default())
                    .unwrap();
                std::io::Write::write_all(&mut zip, b"page").unwrap();
                zip.finish().unwrap();
                let prepared = crate::zip_loader::PreparedZipEnumeration::prepare(
                    path.clone(),
                    crate::zip_loader::enumerate_image_entries_detailed(&path).unwrap(),
                    Default::default(),
                );
                let (deliver, rx) = mpsc::channel();
                zip_complete = Some((deliver, prepared));
                let cancel = Arc::new(AtomicBool::new(false));
                app.zip_enumerate_pending = Some(ZipEnumeratePending {
                    zip_path: path.clone(),
                    input_seq: 1,
                    cancel: Arc::clone(&cancel),
                    rx,
                    restore_intent: StartupListIntent::PageContinuation,
                });
                cancel
            };
            app.fs_nav_after_pdf_enumerate = Some(deferred_navigation_reopen());
            app.fs_nav_locked_gen = Some(app.items_generation);
            app.fs_holdover_tex = Some(FsHoldover::FolderNavigation(None));
            assert!(app.projected_viewer_session_open());
            assert!(!app.whats_new_visible());
            end_waiting_viewer(&mut app, &ctx, close);
            assert!(app.pdf_enumerate_pending.is_none(), "pdf={pdf}, {close}");
            assert!(app.zip_enumerate_pending.is_none(), "pdf={pdf}, {close}");
            assert!(cancel.load(Ordering::Relaxed), "pdf={pdf}, {close}");
            assert!(app.fs_nav_after_pdf_enumerate.is_none());
            assert!(!app.projected_viewer_session_open(), "pdf={pdf}, {close}");
            assert!(app.whats_new_visible(), "pdf={pdf}, {close}");
            let generation = app.items_generation;
            if let Some(complete) = pdf_complete {
                complete(Ok(vec![crate::pdf_loader::PdfPageEntry {
                    page_num: 0,
                    mtime: 1,
                    file_size: 4,
                }]));
            }
            if let Some((deliver, prepared)) = zip_complete {
                assert!(
                    deliver.send(Ok(prepared)).is_err(),
                    "retired receiver: {close}"
                );
            }
            app.poll_pdf_enumerate();
            app.poll_zip_enumerate();
            app.poll_sidecar_restore(&ctx);
            assert_eq!(app.items_generation, generation);
            assert!(app.fullscreen_idx.is_none());
            assert!(app.whats_new_visible());
            assert_eq!(
                app.settings.whats_new_acknowledged_version.as_deref(),
                Some("3.9.0")
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn startup_notices_bulk_settings_close_preserves_list_only_password_request() {
    for detached in [false, true] {
        let mut app = setup_app();
        let path = app.tmp.path().join("list-only.pdf");
        app.pdf_password_request = Some(PdfPasswordRequest::legacy(path.clone()));
        app.pdf_password_input = "list password".into();
        if detached {
            app.build_active_context_for_test(Some(1237), DetachedSource::Image, |owner| {
                owner.items = vec![GridItem::Image(PathBuf::from("sibling.jpg"))];
                owner.fullscreen_idx = Some(0);
                owner.viewer_presentation = ViewerPresentation::DetachedWindow;
            });
            app.with_active_viewer_context(|owner| {
                owner.fs_viewport_shown = true;
                owner.fs_viewport_presentation = Some(ViewerPresentation::DetachedWindow);
            })
            .unwrap();
            // These flags outlive projection changes; they are not main's viewer.
            assert!(app.fs_viewport_shown);
            assert!(app.active_detached_context_is_at_rest());
        }
        assert!(!app.projected_viewer_session_open());
        let ctx = egui::Context::default();
        let scale = app.settings.ui_scale_factor + 0.25;
        app.set_ui_scale_factor(&ctx, scale);
        assert!(!app.active_detached_context_is_at_rest());
        assert!(app.pdf_password_request.is_some(), "detached={detached}");
        assert_eq!(
            app.pdf_password_input, "list password",
            "detached={detached}"
        );
    }
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
        ]
        .into_iter()
        .chain(WAITING_SESSION_END_ROUTES.iter().copied())
        {
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
                // This edge belongs to the sibling's viewport. A root edge must
                // not borrow main's archive continuation to become a viewer key.
                let viewport_id = App::detached_image_window_viewport_id(1235);
                let mut sibling_input = escape_input();
                sibling_input.viewport_id = viewport_id;
                sibling_input
                    .viewports
                    .insert(viewport_id, Default::default());
                sibling_ctx.begin_pass(sibling_input);
                app.with_active_viewer_context(|owner| {
                    assert!(!owner.archive_convert_deferred_fullscreen_active());
                    let action = owner.handle_fs_key_input(&sibling_ctx, 0, false);
                    assert!(
                        action.close,
                        "sibling Escape must be accepted: close={close}, modal={:?}, owner={:?}, events={:?}",
                        owner.modal_dialog_block_reason(),
                        owner.keyboard_owner_for_pass(&sibling_ctx),
                        sibling_ctx.input(|input| input.events.clone()),
                    );
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
            if WAITING_SESSION_END_ROUTES.contains(&close) {
                end_waiting_viewer(&mut app, &ctx, close);
            } else if close.starts_with("backend_") || close.starts_with("internal_reopen_") {
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
