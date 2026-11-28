//! §6.19 f1-f13: real close/scan/guard consumers, recorded viewport commands only.
use super::*;
use crate::ring_shortcut::{RightDragContext, RingActionId, RingShortcutContext};

pub(super) const SURFACE: u64 = 853_190;

fn focus_targets(output: &egui::FullOutput) -> Vec<egui::ViewportId> {
    output
        .viewport_output
        .iter()
        .flat_map(|(id, viewport)| {
            viewport.commands.iter().filter_map(move |command| {
                matches!(command, egui::ViewportCommand::Focus).then_some(*id)
            })
        })
        .collect()
}

pub(super) fn frame(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
    surface: egui::ViewportId,
    key: Option<egui::Key>,
    run: impl FnOnce(),
) -> Vec<egui::ViewportId> {
    ctx.begin_pass(egui::RawInput {
        viewport_id: viewport,
        viewports: [
            (egui::ViewportId::ROOT, egui::ViewportInfo::default()),
            (
                surface,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    focused: (viewport == surface).then_some(true),
                    ..Default::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
        events: key
            .map(|key| egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            })
            .into_iter()
            .collect(),
        ..Default::default()
    });
    run();
    let output = ctx.end_pass();
    let targets = focus_targets(&output);
    output.drop_without_applying_deltas();
    targets
}

fn reader() -> (AppTestEnv, egui::Context, PathBuf, PathBuf) {
    let mut app = setup_app();
    app.active_quick_folder_slot = None;
    let parent = directory(&app, "focus-parent");
    let book = parent.join("book.pdf");
    std::fs::write(&book, b"%PDF-1.4\n").unwrap();
    mount_pdf_reader(&mut app, &book);
    app.fs_focus_grace_elapsed = true;
    app.fs_viewport_shown = true;
    app.settings.fullscreen_keep_on_app_switch = false;
    (app, egui::Context::default(), parent, book)
}

pub(super) fn surface_window(
    app: &App,
    ctx: &egui::Context,
) -> eframe::native_window_input::NativeWindowAllocationLease {
    use eframe::native_window_input::*;
    let id = app.fullscreen_viewport_id();
    ctx.set_embed_viewports(false);
    ctx.run_pass(Default::default(), |ctx| {
        ctx.show_viewport_deferred(id, Default::default(), |_, _| {});
    })
    .drop_without_applying_deltas();
    register_native_window_allocation(
        ctx,
        NativeWindowAllocation {
            viewport_id: id,
            registration: ctx.viewport_registration(id).unwrap(),
            window_id: NativeWindowId::from(SURFACE),
            windows_hwnd: Some(SURFACE as isize),
        },
    )
}

pub(super) fn close_entry(app: &mut App, ctx: &egui::Context, entry: &str) {
    if entry == "ring" {
        app.apply_ring_action(
            ctx,
            RingShortcutContext::ImageFullscreen,
            RingActionId::CloseFullscreen,
            "section1853-close-focus",
        );
        return;
    }
    let close = if entry == "right-click" {
        app.apply_viewer_short_right_click_action(
            RightDragContext::ImageFullscreen,
            Some(0),
            egui::pos2(100.0, 100.0),
        )
    } else {
        app.handle_fs_key_input(ctx, 0, false).close
    };
    assert!(close, "{entry} must reach the ordinary close handler");
    app.handle_fs_navigation(
        ctx,
        close,
        false,
        None,
        None,
        None,
        crate::ui_fullscreen::FsPageNav::None,
        None,
        0,
    );
}

pub(super) fn dispatch_close(app: &mut App, ctx: &egui::Context) {
    let nav = app
        .take_pending_return_to_parent_nav_with_focus(ctx)
        .unwrap();
    assert!(app.apply_fullscreen_close_nav_immediate_with_focus(ctx, nav));
}

#[test]
fn section1853_close_focus_entries_discard_focus_for_c5_in_request_frame() {
    for (entry, key) in [
        ("enter", Some(egui::Key::Enter)),
        ("escape", Some(egui::Key::Escape)),
        ("right-click", None),
        ("ring", None),
    ] {
        for child in [false, true] {
            let (mut app, ctx, _, book) = reader();
            let _window = surface_window(&app, &ctx);
            let surface = app.fullscreen_viewport_id();
            let targets = frame(
                &ctx,
                if child {
                    surface
                } else {
                    egui::ViewportId::ROOT
                },
                surface,
                key,
                || {
                    close_entry(&mut app, &ctx, entry);
                    assert!(app.pending_return_to_parent.is_pending());
                    dispatch_close(&mut app, &ctx);
                    // A second seam poll in the same frame cannot repeat Focus.
                    assert!(
                        app.take_pending_return_to_parent_nav_with_focus(&ctx)
                            .is_none()
                    );
                },
            );
            assert!(targets.is_empty(), "{entry}, child={child}: {targets:?}");
            assert_eq!(app.current_folder.as_ref(), Some(&book));
            assert_eq!(app.fullscreen_idx, Some(0));
            assert!(app.direct_navigation_scan_pending());
            app.cancel_folder_pane_open(PaneOpenRestoreExit::Abandoned);
        }
    }
}

#[test]
fn section1853_close_focus_slow_parent_guard_restores_then_adopts_once() {
    for keep in [false, true] {
        let (mut app, ctx, parent, book) = reader();
        let _window = surface_window(&app, &ctx);
        let _foreground = crate::test_native_focus::scoped(SURFACE, 0, true);
        let surface = app.fullscreen_viewport_id();
        app.settings.fullscreen_keep_on_app_switch = keep;
        assert!(
            frame(
                &ctx,
                egui::ViewportId::ROOT,
                surface,
                Some(egui::Key::Enter),
                || {
                    close_entry(&mut app, &ctx, "enter");
                    dispatch_close(&mut app, &ctx);
                }
            )
            .is_empty()
        );
        let (tx, result, token) = hold_scan_reply(&mut app);
        let generation = app.items_generation;
        let targets = frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
            app.reconcile_fullscreen_after_main_focus(&ctx, true, false, false);
            app.poll_main_embedded_required_fullscreen_folder_open(&ctx);
        });
        assert_eq!(targets, [surface]);
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.current_folder.as_ref(), Some(&book));
        assert_eq!(app.items_generation, generation);
        assert!(!token.load(Ordering::Relaxed));
        assert!(app.fs_nav_is_locked());
        tx.send(result).unwrap();
        let targets = frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
            app.poll_main_embedded_required_fullscreen_folder_open(&ctx);
            app.post_render_main_navigation(&ctx, Default::default());
        });
        assert_eq!(targets, [egui::ViewportId::ROOT]);
        assert_eq!(app.fullscreen_idx, None);
        assert_eq!(app.current_folder.as_ref(), Some(&parent));
        assert!(
            matches!(&app.items[app.selected.unwrap()], GridItem::PdfFile(path) if path == &book)
        );
        assert!(!app.fs_nav_is_locked());
        assert!(
            frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
                app.poll_main_embedded_required_fullscreen_folder_open(&ctx);
                app.post_render_main_navigation(&ctx, Default::default());
            })
            .is_empty()
        );
    }
}

#[test]
fn section1853_close_focus_fast_parent_ordinary_tail_keeps_selection_and_one_root_focus() {
    let (mut app, ctx, parent, book) = reader();
    let _window = surface_window(&app, &ctx);
    let _foreground = crate::test_native_focus::scoped(SURFACE, 0, true);
    let surface = app.fullscreen_viewport_id();
    assert!(
        frame(
            &ctx,
            egui::ViewportId::ROOT,
            surface,
            Some(egui::Key::Enter),
            || {
                close_entry(&mut app, &ctx, "enter");
                dispatch_close(&mut app, &ctx);
            }
        )
        .is_empty()
    );
    let (tx, result, _) = hold_scan_reply(&mut app);
    tx.send(result).unwrap();
    let targets = frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
        app.post_render_main_navigation(&ctx, Default::default());
    });
    assert_eq!(targets, [egui::ViewportId::ROOT]);
    assert_eq!(app.fullscreen_idx, None);
    assert_eq!(app.current_folder.as_ref(), Some(&parent));
    assert!(matches!(&app.items[app.selected.unwrap()], GridItem::PdfFile(path) if path == &book));
}

#[test]
fn section1853_close_focus_stale_focused_sibling_foreign_and_unknown_never_claim_root() {
    for (foreground, owned) in [(853_191, true), (853_192, false), (0, false)] {
        let (mut app, ctx, parent, _) = reader();
        let _window = surface_window(&app, &ctx);
        let surface = app.fullscreen_viewport_id();
        assert!(
            frame(&ctx, surface, surface, Some(egui::Key::Enter), || {
                close_entry(&mut app, &ctx, "enter");
                dispatch_close(&mut app, &ctx);
            })
            .is_empty()
        );
        let (tx, result, _) = hold_scan_reply(&mut app);
        let _foreground = crate::test_native_focus::scoped(foreground, 0, owned);
        // This is deliberately stale and remains true during the ROOT-only frame.
        assert_eq!(ctx.input_for(surface, |i| i.viewport().focused), Some(true));
        tx.send(result).unwrap();
        let targets = frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
            assert_eq!(ctx.input_for(surface, |i| i.viewport().focused), Some(true));
            app.poll_main_embedded_required_fullscreen_folder_open(&ctx);
        });
        assert!(targets.is_empty());
        assert_eq!(app.fullscreen_idx, None);
        assert_eq!(app.current_folder.as_ref(), Some(&parent));
    }
}

#[test]
fn section1853_close_focus_embedded_and_missing_surface_never_claim_root_at_adoption() {
    for embedded in [false, true] {
        let (mut app, ctx, parent, _) = reader();
        let _foreground = crate::test_native_focus::scoped(SURFACE, 0, true);
        app.native_video_in_window_active = embedded;
        let surface = app.fullscreen_viewport_id();
        // No allocation witness for the non-embedded case: unknown is fail-closed.
        let _window = embedded.then(|| surface_window(&app, &ctx));
        assert!(
            frame(
                &ctx,
                egui::ViewportId::ROOT,
                surface,
                Some(egui::Key::Enter),
                || {
                    close_entry(&mut app, &ctx, "enter");
                    dispatch_close(&mut app, &ctx);
                }
            )
            .is_empty()
        );
        let (tx, result, _) = hold_scan_reply(&mut app);
        tx.send(result).unwrap();
        assert!(
            frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
                app.poll_main_embedded_required_fullscreen_folder_open(&ctx);
            })
            .is_empty()
        );
        assert_eq!(app.fullscreen_idx, None);
        assert_eq!(app.current_folder.as_ref(), Some(&parent));
    }
}

#[test]
fn section1853_close_focus_true_close_preserves_entry_frame_and_viewport_target() {
    for child in [false, true] {
        for entry in ["enter", "ring"] {
            let (mut app, ctx, _, _) = reader();
            app.settings.auto_fullscreen_zip_pdf = false;
            let surface = app.fullscreen_viewport_id();
            let target = if child {
                surface
            } else {
                egui::ViewportId::ROOT
            };
            let targets = frame(&ctx, target, surface, Some(egui::Key::Enter), || {
                close_entry(&mut app, &ctx, entry);
                assert!(
                    app.take_pending_return_to_parent_nav_with_focus(&ctx)
                        .is_none()
                );
            });
            assert_eq!(targets, [target], "{entry}, child={child}");
            assert_eq!(app.fullscreen_idx, None);
        }
    }
}

#[test]
fn section1853_close_focus_error_disconnect_cancel_and_refusal_keep_reader_without_focus() {
    for terminal in ["error", "disconnect", "cancel", "refusal"] {
        let (mut app, ctx, _, book) = reader();
        let _window = surface_window(&app, &ctx);
        let _foreground = crate::test_native_focus::scoped(SURFACE, 0, true);
        let surface = app.fullscreen_viewport_id();
        assert!(
            frame(
                &ctx,
                egui::ViewportId::ROOT,
                surface,
                Some(egui::Key::Enter),
                || {
                    close_entry(&mut app, &ctx, "enter");
                    dispatch_close(&mut app, &ctx);
                }
            )
            .is_empty()
        );
        let (tx, result, _) = hold_scan_reply(&mut app);
        match terminal {
            "error" => tx
                .send(FolderOpenScanResult {
                    scan: Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
                    policy: app.folder_scan_policy(),
                    policy_mixed: false,
                })
                .unwrap(),
            "disconnect" => drop(tx),
            "cancel" => {
                app.cancel_folder_pane_open(PaneOpenRestoreExit::Abandoned);
                assert!(tx.send(result).is_err());
            }
            "refusal" => {
                // Ordinary modal admission refusal at resume, after a successful scan.
                let _operation_tx = app.hold_book_structure_for_test();
                assert!(app.document_open_modal_admission_blocked());
                tx.send(result).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
                app.poll_main_embedded_required_fullscreen_folder_open(&ctx);
            })
            .is_empty(),
            "{terminal}"
        );
        assert_eq!(app.fullscreen_idx, Some(0), "{terminal}");
        assert_eq!(app.current_folder.as_ref(), Some(&book));
        assert!(!app.fs_nav_is_locked(), "{terminal}");
        assert!(!app.direct_navigation_scan_pending(), "{terminal}");
    }
}

#[test]
fn section1853_close_focus_dispatch_refusal_before_worker_keeps_viewer_without_focus() {
    for ordinary_tail in [false, true] {
        let (mut app, ctx, _, book) = reader();
        let surface = app.fullscreen_viewport_id();
        assert!(
            frame(
                &ctx,
                egui::ViewportId::ROOT,
                surface,
                Some(egui::Key::Enter),
                || {
                    close_entry(&mut app, &ctx, "enter");
                    let nav = app
                        .take_pending_return_to_parent_nav_with_focus(&ctx)
                        .unwrap();
                    let _operation = app.hold_book_structure_for_test();
                    if ordinary_tail {
                        app.post_render_main_navigation(
                            &ctx,
                            update_phases::MainFrameNavigation {
                                fullscreen_close_nav: Some(nav),
                                ..Default::default()
                            },
                        );
                    } else {
                        assert!(app.apply_fullscreen_close_nav_immediate_with_focus(&ctx, nav));
                    }
                }
            )
            .is_empty()
        );
        assert_eq!(app.fullscreen_idx, Some(0));
        assert_eq!(app.current_folder.as_ref(), Some(&book));
        assert!(!app.fs_close_return_navigation_wait_active());
        assert!(!app.direct_navigation_scan_pending());
        assert!(
            frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
                assert!(
                    app.take_pending_return_to_parent_nav_with_focus(&ctx)
                        .is_none()
                );
            })
            .is_empty()
        );
    }
}

#[test]
fn section1853_close_focus_main_and_native_tails_decide_from_started_c5_wait() {
    for native_tail in [false, true] {
        let (mut app, ctx, parent, _) = reader();
        let _window = surface_window(&app, &ctx);
        let _foreground = crate::test_native_focus::scoped(SURFACE, 0, true);
        let surface = app.fullscreen_viewport_id();
        assert!(
            frame(
                &ctx,
                egui::ViewportId::ROOT,
                surface,
                Some(egui::Key::Enter),
                || {
                    close_entry(&mut app, &ctx, "enter");
                    let nav = app
                        .take_pending_return_to_parent_nav_with_focus(&ctx)
                        .unwrap();
                    if native_tail {
                        app.dispatch_native_backdrop_navigation(&ctx, Some(nav), true, None);
                    } else {
                        app.post_render_main_navigation(
                            &ctx,
                            update_phases::MainFrameNavigation {
                                fullscreen_close_nav: Some(nav),
                                ..Default::default()
                            },
                        );
                    }
                }
            )
            .is_empty()
        );
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(app.fs_close_return_navigation_wait_active());
        let (tx, result, _) = hold_scan_reply(&mut app);
        tx.send(result).unwrap();
        assert_eq!(
            frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
                if native_tail {
                    app.dispatch_native_backdrop_navigation(&ctx, None, false, None);
                } else {
                    app.post_render_main_navigation(&ctx, Default::default());
                }
            }),
            [egui::ViewportId::ROOT]
        );
        assert_eq!(app.current_folder.as_ref(), Some(&parent));
        assert_eq!(app.fullscreen_idx, None);
    }
}

#[test]
fn section1853_close_focus_losing_return_drops_focus_in_same_frame_even_if_winner_refused() {
    let (mut app, ctx, _, book) = reader();
    let surface = app.fullscreen_viewport_id();
    let other = directory(&app, "focus-higher-priority");
    assert!(
        frame(
            &ctx,
            egui::ViewportId::ROOT,
            surface,
            Some(egui::Key::Enter),
            || {
                close_entry(&mut app, &ctx, "enter");
                let nav = app
                    .take_pending_return_to_parent_nav_with_focus(&ctx)
                    .unwrap();
                let _operation = app.hold_book_structure_for_test();
                app.post_render_main_navigation(
                    &ctx,
                    update_phases::MainFrameNavigation {
                        toolbar_fav_nav: Some(other),
                        fullscreen_close_nav: Some(nav),
                        ..Default::default()
                    },
                );
            }
        )
        .is_empty()
    );
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.current_folder.as_ref(), Some(&book));
    assert!(
        frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
            assert!(
                app.take_pending_return_to_parent_nav_with_focus(&ctx)
                    .is_none()
            );
            app.post_render_main_navigation(&ctx, Default::default());
        })
        .is_empty()
    );
}

#[test]
fn section1853_close_focus_second_close_backspace_navigation_and_remote_drop_old_authority() {
    for replacement in ["second-close", "backspace", "navigation", "remote"] {
        let (mut app, ctx, parent, book) = reader();
        let _window = surface_window(&app, &ctx);
        let _foreground = crate::test_native_focus::scoped(SURFACE, 0, true);
        let surface = app.fullscreen_viewport_id();
        assert!(
            frame(
                &ctx,
                egui::ViewportId::ROOT,
                surface,
                Some(egui::Key::Enter),
                || {
                    close_entry(&mut app, &ctx, "enter");
                    dispatch_close(&mut app, &ctx);
                }
            )
            .is_empty()
        );
        let (late_tx, late_result, token) = hold_scan_reply(&mut app);
        let targets = frame(
            &ctx,
            egui::ViewportId::ROOT,
            surface,
            Some(egui::Key::Enter),
            || match replacement {
                "second-close" => {
                    close_entry(&mut app, &ctx, "enter");
                    dispatch_close(&mut app, &ctx);
                }
                "backspace" => app.handle_fs_navigation(
                    &ctx,
                    false,
                    true,
                    None,
                    None,
                    None,
                    crate::ui_fullscreen::FsPageNav::None,
                    None,
                    0,
                ),
                "navigation" => {
                    let other = directory(&app, "focus-new-navigation");
                    app.app.load_folder(other);
                    app.cancel_folder_pane_open(PaneOpenRestoreExit::Abandoned);
                }
                "remote" => {
                    app.invalidate_local_viewer_open_intents_for_remote();
                }
                _ => unreachable!(),
            },
        );
        assert!(token.load(Ordering::Relaxed));
        assert!(late_tx.send(late_result).is_err());
        if replacement == "second-close" {
            assert!(targets.is_empty());
            let (tx, result, _) = hold_scan_reply(&mut app);
            tx.send(result).unwrap();
            assert_eq!(
                frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
                    app.poll_main_embedded_required_fullscreen_folder_open(&ctx);
                }),
                [egui::ViewportId::ROOT]
            );
            assert_eq!(app.current_folder.as_ref(), Some(&parent));
            assert!(
                matches!(&app.items[app.selected.unwrap()], GridItem::PdfFile(path) if path == &book)
            );
        } else {
            assert_eq!(
                targets,
                if replacement == "backspace" {
                    vec![egui::ViewportId::ROOT]
                } else {
                    vec![]
                }
            );
            assert!(
                frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
                    app.poll_main_embedded_required_fullscreen_folder_open(&ctx);
                    app.post_render_main_navigation(&ctx, Default::default());
                })
                .is_empty()
            );
        }
    }
}

#[test]
fn section1853_close_focus_guard_ctrl_folder_wait_and_existing_gates_are_unchanged() {
    for keep in [false, true] {
        let (mut app, ctx, _, _) = reader();
        let surface = app.fullscreen_viewport_id();
        app.settings.fullscreen_keep_on_app_switch = keep;
        app.begin_fs_folder_navigation_sequence(&ctx, 0);
        assert!(!app.fs_close_return_navigation_wait_active());
        let targets = frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
            app.reconcile_fullscreen_after_main_focus(&ctx, true, false, false);
        });
        assert_eq!(app.fullscreen_idx, keep.then_some(0));
        assert_eq!(targets, if keep { vec![surface] } else { vec![] });
    }
    // Preserve the intentional native-owner asymmetry: it blocks close, not restore.
    for grace in [false, true] {
        for focused in [false, true] {
            for embedded in [false, true] {
                for root_key in [false, true] {
                    for native_owner in [false, true] {
                        let keep = true; // C5 or the existing keep setting.
                        assert!(!should_close_fullscreen_from_main_focus(
                            grace,
                            focused,
                            native_owner,
                            embedded,
                            keep,
                            root_key
                        ));
                        assert_eq!(
                            should_restore_fullscreen_focus_from_main_focus(
                                grace, focused, embedded, keep, root_key
                            ),
                            grace && focused && !embedded && !root_key
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn section1853_close_focus_native_deferred_terminal_and_immediate_have_one_effect_owner() {
    for immediate in [false, true] {
        let (mut app, ctx, _, _) = reader();
        app.settings.auto_fullscreen_zip_pdf = false;
        app.items[0] = GridItem::Video(app.tmp.path().join("native.mp4"));
        let _window = surface_window(&app, &ctx);
        let _foreground = crate::test_native_focus::scoped(SURFACE, 0, true);
        let surface = app.fullscreen_viewport_id();
        app.video_presentation_transition
            .reset_stable(ViewerPresentation::Fullscreen);
        app.video_presentation_transition.request_transition(
            ViewerPresentation::MainWindow,
            false,
            false,
            0,
            None,
            None,
        );
        app.video_presentation_transition.take_effects();
        app.video_presentation_transition.drive();
        let targets = frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
            let outcome = if immediate {
                app.handle_fullscreen_close_request_immediate()
            } else {
                app.handle_fullscreen_close_request()
            };
            assert_eq!(
                outcome,
                FullscreenCloseRequestOutcome::DeferredTerminalClose
            );
            app.route_fullscreen_close_entry_focus(&ctx, outcome, true, "native-deferred-test");
            assert_eq!(app.fullscreen_idx, Some(0));
        });
        assert!(targets.is_empty());
        let effects = app.video_presentation_transition.take_effects();
        assert_eq!(
            effects
                .iter()
                .filter(|effect| matches!(
                    effect,
                    PresentationTransitionEffect::TerminalSessionClose { .. }
                ))
                .count(),
            1
        );
        assert!(
            frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
                let mut closes = 0;
                // No native renderer/placement was created by this headless
                // fixture. Deliver the real terminal host effect, as in d4.
                for effect in effects.into_iter().filter(|effect| {
                    matches!(
                        effect,
                        PresentationTransitionEffect::TerminalSessionClose { .. }
                    )
                }) {
                    if app.execute_video_presentation_host_effect(&ctx, effect)
                        == PresentationEffectsOutcome::ClosedFullscreen
                    {
                        closes += 1;
                    }
                }
                assert_eq!(closes, 1);
                assert_eq!(app.fullscreen_idx, None);
                // Existing native reclaim is scheduled by teardown; generic Focus
                // does not compete with it, even when another effect poll follows.
                assert!(app.pending_main_foreground_reclaim);
                assert_eq!(
                    app.execute_video_presentation_transition_effects(&ctx),
                    PresentationEffectsOutcome::DidNotClose
                );
            })
            .is_empty()
        );
        assert!(crate::test_native_focus::requests().is_empty());
    }
}

#[test]
fn section1853_close_focus_video_ring_keeps_page_list_and_native_reclaim_owner() {
    let (mut app, ctx, _, book) = reader();
    app.items[0] = GridItem::Video(app.tmp.path().join("native.mp4"));
    let _foreground = crate::test_native_focus::scoped(SURFACE, 0, true);
    let surface = app.fullscreen_viewport_id();
    assert!(
        frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
            app.apply_ring_action(
                &ctx,
                RingShortcutContext::VideoFullscreen,
                RingActionId::CloseFullscreen,
                "video-ring-test",
            );
        })
        .is_empty()
    );
    assert_eq!(app.fullscreen_idx, None);
    assert_eq!(app.current_folder.as_ref(), Some(&book));
    assert!(!app.direct_navigation_scan_pending());
    assert!(app.pending_main_foreground_reclaim);
}

#[test]
fn section1853_close_focus_rating_back_worker_keeps_entry_focus_and_existing_guard_policy() {
    for child in [false, true] {
        for main_focus in [false, true] {
            let (mut app, ctx, _, book) = reader();
            app.rating_view_nav_stack = vec![book];
            app.rating_view_stars = 3;
            let surface = app.fullscreen_viewport_id();
            let target = if child {
                surface
            } else {
                egui::ViewportId::ROOT
            };
            assert_eq!(
                frame(&ctx, target, surface, Some(egui::Key::Enter), || {
                    close_entry(&mut app, &ctx, "enter");
                    let nav = app
                        .take_pending_return_to_parent_nav_with_focus(&ctx)
                        .unwrap();
                    assert!(matches!(nav, crate::ui_main::AddressBarNav::RatingViewBack));
                    assert!(app.apply_fullscreen_close_nav_immediate_with_focus(&ctx, nav));
                }),
                vec![target]
            );
            assert!(app.rating_view_pending.is_some());
            assert_eq!(app.fullscreen_idx, Some(0));
            assert!(!app.fs_close_return_navigation_wait_active());
            // The pre-C5 worker sends in the entry frame, never in a later frame.
            assert!(frame(&ctx, target, surface, None, || {}).is_empty());
            // Its released guard policy is also unchanged: main focus closes it.
            if main_focus {
                assert!(
                    frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
                        app.reconcile_fullscreen_after_main_focus(&ctx, true, false, false);
                    })
                    .is_empty()
                );
                assert_eq!(app.fullscreen_idx, None);
            }
            let deadline = Instant::now() + Duration::from_secs(30);
            while app.rating_view_pending.is_some() || app.sidecar_restore_active() {
                assert!(
                    frame(&ctx, target, surface, None, || {
                        app.poll_rating_view();
                        app.poll_sidecar_restore(&ctx);
                    })
                    .is_empty()
                );
                assert!(Instant::now() < deadline, "rating root did not adopt");
                std::thread::yield_now();
            }
            assert!(app.items_are_rating_view);
            assert_eq!(app.fullscreen_idx, None);
        }
    }
}

#[test]
fn section1853_close_focus_rating_physical_restore_keeps_entry_frame_and_no_adoption_focus() {
    for child in [false, true] {
        for classify in [false, true] {
            let (mut app, ctx, parent, mut book) = reader();
            // A directory with an archive suffix also exercises the existing
            // classification owner before physical-history preflight.
            let parent = if classify {
                let path = parent.join("physical-parent.zip");
                std::fs::create_dir(&path).unwrap();
                let moved_book = path.join("book.pdf");
                std::fs::rename(&book, &moved_book).unwrap();
                book = moved_book;
                mount_pdf_reader(&mut app, &book);
                path
            } else {
                parent
            };
            app.rating_view_nav_stack = vec![parent.clone(), book];
            app.rating_view_stars = 3;
            let surface = app.fullscreen_viewport_id();
            let target = if child {
                surface
            } else {
                egui::ViewportId::ROOT
            };
            assert_eq!(
                frame(&ctx, target, surface, Some(egui::Key::Enter), || {
                    close_entry(&mut app, &ctx, "enter");
                    dispatch_close(&mut app, &ctx);
                }),
                vec![target]
            );
            assert!(app.viewer_session_blocks_main_window());
            assert!(!app.fs_close_return_navigation_wait_active());
            let deadline = Instant::now() + Duration::from_secs(30);
            while app.viewer_session_blocks_main_window() {
                assert!(
                    frame(&ctx, target, surface, None, || {
                        app.settle_open_path_classification_for_test();
                        app.poll_sidecar_restore(&ctx);
                        app.poll_collection_history_transition(&ctx);
                    })
                    .is_empty()
                );
                assert!(Instant::now() < deadline, "rating parent did not adopt");
                std::thread::yield_now();
            }
            assert_eq!(app.current_folder.as_ref(), Some(&parent));
            assert_eq!(app.rating_view_nav_stack, vec![parent]);
        }
    }
}

#[test]
fn section1853_close_focus_rating_refusal_cannot_borrow_existing_worker_focus() {
    let (mut app, ctx, _, book) = reader();
    app.rating_view_nav_stack = vec![book];
    app.rating_view_stars = 3;
    app.rating_view_back();
    let sequence = app.rating_view_pending.as_ref().unwrap().sequence;
    let surface = app.fullscreen_viewport_id();
    // A second dispatch refused before admission leaves the old worker intact.
    assert!(
        frame(&ctx, egui::ViewportId::ROOT, surface, None, || {
            let outcome = app.handle_fullscreen_close_request();
            app.route_fullscreen_close_entry_focus(&ctx, outcome, true, "rating-refusal-test");
            let nav = app
                .take_pending_return_to_parent_nav_with_focus(&ctx)
                .unwrap();
            let _operation = app.hold_book_structure_for_test();
            assert!(app.apply_fullscreen_close_nav_immediate_with_focus(&ctx, nav));
        })
        .is_empty()
    );
    assert_eq!(app.rating_view_pending.as_ref().unwrap().sequence, sequence);
    assert!(app.viewer_session_blocks_main_window());
}

#[test]
fn section1853_close_focus_subfolder_return_keeps_entry_frame_and_no_adoption_focus() {
    use crate::app::subfolder_expansion::{
        SubfolderExpansionRestoreState, SubfolderExpansionSnapshot,
    };
    for child in [false, true] {
        for prepared_snapshot in [false, true] {
            let (mut app, ctx, parent, _) = reader();
            let snapshot = prepared_snapshot.then(|| SubfolderExpansionSnapshot {
                file_type_visibility: app.settings.file_type_visibility.clone(),
                root: parent.clone(),
                roots: vec![parent.clone()],
                scan_filter: Default::default(),
                entries: Arc::new(vec![]),
                video_thumb_overrides: Default::default(),
                diag: Default::default(),
                stack_rating_source: Default::default(),
                stack_order_request: crate::rating_sort::ListingOrderRequest::Standard(
                    crate::settings::SortOrder::FileName,
                ),
            });
            // A Rating root can restore its captured expansion origin through
            // the same legacy asynchronous Return owner (scan or prepare).
            app.items_are_rating_view = true;
            app.rating_view_return_to = Some(Box::new(top_level_grid_view::TopLevelGridOrigin {
                restore: top_level_grid_view::TopLevelGridRestore::SubfolderExpansion(
                    SubfolderExpansionRestoreState {
                        root: Some(parent.clone()),
                        roots: vec![parent.clone()],
                        saved_folder: Some(parent),
                        snapshot,
                        removed_paths: Default::default(),
                    },
                ),
                route: FacetRoute::root(FacetScope::path(&subfolder_expansion_synthetic_path())),
            }));
            let surface = app.fullscreen_viewport_id();
            let target = if child {
                surface
            } else {
                egui::ViewportId::ROOT
            };
            assert_eq!(
                frame(&ctx, target, surface, Some(egui::Key::Enter), || {
                    close_entry(&mut app, &ctx, "enter");
                    dispatch_close(&mut app, &ctx);
                }),
                vec![target]
            );
            assert!(app.viewer_session_blocks_main_window());
            assert!(!app.fs_close_return_navigation_wait_active());
            let deadline = Instant::now() + Duration::from_secs(30);
            while app.viewer_session_blocks_main_window() {
                assert!(
                    frame(&ctx, target, surface, None, || {
                        app.poll_subfolder_expansion(&ctx);
                        app.poll_sidecar_restore(&ctx);
                    })
                    .is_empty()
                );
                assert!(Instant::now() < deadline, "expansion return did not adopt");
                std::thread::yield_now();
            }
            assert!(app.items_are_subfolder_expansion_view);
            assert_eq!(
                app.current_folder,
                Some(subfolder_expansion_synthetic_path())
            );
        }
    }
}
