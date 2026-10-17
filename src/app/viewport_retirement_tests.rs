//! App owner to exact root command handoff; native adapter cleanup is tested in eframe.
use super::*;

fn register(app: &mut App, ctx: &egui::Context, ids: &[u64]) -> Vec<egui::ViewportRegistration> {
    ctx.set_embed_viewports(false);
    let output = ctx.run_ui(Default::default(), |ui| {
        for &id in ids {
            ctx.show_viewport_deferred(
                App::detached_image_window_viewport_id(id),
                egui::ViewportBuilder::default(),
                |_, _| {},
            );
            app.capture_detached_viewport_registration(ui.ctx(), id);
        }
    });
    output.drop_without_applying_deltas();
    ids.iter()
        .map(|id| {
            ctx.viewport_registration(App::detached_image_window_viewport_id(*id))
                .unwrap()
        })
        .collect()
}

fn approvals(output: &egui::FullOutput) -> Vec<egui::ViewportRegistration> {
    output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .into_iter()
        .flat_map(|root| &root.commands)
        .filter_map(|command| match command {
            egui::ViewportCommand::RetireApproved(approval) => Some(approval.registrations()),
            _ => None,
        })
        .flatten()
        .copied()
        .collect()
}

#[test]
fn viewport_retirement_mode_change_transfers_active_parked_and_passive_exact_lifetimes() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.build_active_context_for_test(Some(93001), DetachedSource::Image, |_| {});
    app.push_window_context_for_test(&ctx, 93002, |_| {});
    app.push_window_context_for_test(&ctx, 93003, |_| {});
    for id in [93001, 93002, 93003] {
        app.update_detached_window_runtime_flags(id, false, "retirement_test");
    }
    let registered = register(&mut app, &ctx, &[93001, 93002, 93003]);
    assert!(
        app.close_all_detached_viewers_for_mode_change(&ctx)
            .unwrap()
    );
    assert!(app.detached_window_manager.is_empty());
    assert!(app.active_detached_session.is_none());
    assert!(ctx.has_pending_viewport_retirements());
    assert_eq!(
        app.local_viewer_close_state(&ctx, None),
        LocalViewerCloseState::InProgress
    );
    let output = ctx.run_ui(Default::default(), |_| {});
    let transferred = approvals(&output);
    let synthetic = output.viewport_output.values().any(|viewport| {
        viewport.commands.iter().any(|command| {
            matches!(
                command,
                egui::ViewportCommand::Close | egui::ViewportCommand::Visible(false)
            )
        })
    });
    output.drop_without_applying_deltas();
    for registration in registered {
        assert!(transferred.contains(&registration));
    }
    assert!(!synthetic);
}

#[test]
fn viewport_retirement_context_gc_preserves_app_binding_authority() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.update_detached_window_runtime_flags(93004, false, "retirement_test");
    let registered = register(&mut app, &ctx, &[93004])[0];
    // A real end_pass GC removes the Context child before the App owner ends.
    ctx.run_ui(Default::default(), |_| {})
        .drop_without_applying_deltas();
    assert!(
        ctx.viewport_registration(registered.viewport_id())
            .is_none()
    );
    app.retire_terminal_detached_viewport_identity(93004, "retirement_after_gc");
    let output = ctx.run_ui(Default::default(), |_| {});
    let transferred = approvals(&output);
    output.drop_without_applying_deltas();
    assert!(transferred.contains(&registered));
    assert!(
        ctx.viewport_registration(registered.viewport_id())
            .is_none()
    );
}

#[test]
fn viewport_retirement_unregistered_owner_does_not_create_a_viewport() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.update_detached_window_runtime_flags(93005, false, "retirement_test");
    app.retire_terminal_detached_viewport_identity(93005, "retirement_before_show");
    assert!(!ctx.has_pending_viewport_retirements());
    assert!(
        ctx.viewport_registration(App::detached_image_window_viewport_id(93005))
            .is_none()
    );
    assert!(app.detached_window_manager.is_empty());
}

#[test]
fn viewport_retirement_repeat_terminal_owner_transfers_once() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.update_detached_window_runtime_flags(93006, false, "retirement_test");
    let registration = register(&mut app, &ctx, &[93006])[0];
    app.retire_terminal_detached_viewport_identity(93006, "first");
    app.retire_terminal_detached_viewport_identity(93006, "repeat");
    let output = ctx.run_ui(Default::default(), |_| {});
    let transferred = approvals(&output);
    output.drop_without_applying_deltas();
    assert_eq!(
        transferred
            .iter()
            .filter(|&&value| value == registration)
            .count(),
        1
    );
}

#[test]
fn viewport_retirement_parking_keeps_registration_and_worker_owner() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.update_detached_window_runtime_flags(93007, false, "retirement_test");
    let registration = register(&mut app, &ctx, &[93007])[0];
    app.transition_detached_window_state(93007, DetachedWindowState::Parked, "parking");
    assert!(!ctx.has_pending_viewport_retirements());
    assert_eq!(
        ctx.viewport_registration(registration.viewport_id()),
        Some(registration)
    );
    assert_eq!(
        app.detached_window_state(93007),
        Some(DetachedWindowState::Parked)
    );
}

#[test]
fn viewport_retirement_managed_hook_consumes_modal_reject_then_accepts_new_close() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.build_active_context_for_test(Some(93008), DetachedSource::Image, |_| {});
    app.update_detached_window_runtime_flags(93008, false, "retirement_test");
    let registration = register(&mut app, &ctx, &[93008])[0];
    let folder = app.tmp.path().join("book");
    app.with_window_viewer_context_state(93008, |owner| {
        owner.activate_sidecar_restore_modal_for_test(folder);
    })
    .unwrap();
    assert_eq!(app.sidecar_restore_target_window_id(), Some(93008));
    assert_eq!(
        eframe::App::on_viewport_close_requested(&mut app.app, &ctx, registration.viewport_id()),
        eframe::ViewportCloseResponse::Cancel
    );
    assert!(!ctx.has_pending_viewport_retirements());
    assert!(app.locate_window_context(93008).is_some());
    let unknown = egui::ViewportId::from_hash_of("non-managed-tool");
    assert_eq!(
        eframe::App::on_viewport_close_requested(&mut app.app, &ctx, unknown),
        eframe::ViewportCloseResponse::DelegateToUi
    );
    // End the unrelated modal through its existing test owner before terminal close.
    assert!(app.service_discarded_sidecar_restore(&ctx));
    assert_eq!(
        eframe::App::on_viewport_close_requested(&mut app.app, &ctx, registration.viewport_id()),
        eframe::ViewportCloseResponse::RetirementTransferred
    );
    assert!(ctx.has_pending_viewport_retirements());
    assert!(app.detached_window_manager.is_empty());
    assert!(app.locate_window_context(93008).is_none());
}

#[test]
fn egui037_continuation_viewport_retirement_remote_close_retains_pin_dirty_for_grid_ui() {
    use crate::remote_ipc::session::{RemoteControlPhase, SessionHandle};
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let video = app.tmp.path().join("dirty-video.mp4");
    let folder = app.tmp.path().join("dirty-folder");
    app.items = vec![GridItem::Video(video.clone())];
    app.thumbnails = vec![crate::grid_item::ThumbnailState::Pending];
    app.image_metas = vec![None];
    app.visible_indices = vec![0];
    app.selected = Some(0);
    app.fullscreen_idx = Some(0);
    app.current_folder = Some(app.tmp.path().to_path_buf());
    app.folder_thumb_pin_dirty.insert(folder.clone());
    app.video_thumb_overrides_dirty_paths.insert(video.clone());
    let frame = app.frame_counter;
    let sequence = app.input_seq;
    let session = SessionHandle::new();
    app.set_remote_session_handle(session.clone());
    session.acquire(mimageviewer_ipc::SessionAcquireRequest {
        client_id: "retirement-pin-dirty".to_owned(),
        peer: mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: None,
        },
    });
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx);
    for _ in 0..2 {
        let (_output, _) = backend.update_logic_only(&mut app.app, Default::default());
        assert_eq!(session.snapshot().phase, RemoteControlPhase::RemoteActive);
        assert!(app.fullscreen_idx.is_none());
        assert_eq!(app.folder_thumb_pin_dirty.len(), 1);
        assert!(app.folder_thumb_pin_dirty.contains(&folder));
        assert_eq!(app.video_thumb_overrides_dirty_paths.len(), 1);
        assert!(app.video_thumb_overrides_dirty_paths.contains(&video));
        assert!(matches!(app.items.as_slice(), [GridItem::Video(path)] if path == &video));
        assert_eq!(app.frame_counter, frame);
        assert_eq!(app.input_seq, sequence);
    }
}

#[test]
fn viewport_retirement_remote_logic_waits_for_command_delivery_and_existing_save_ack() {
    use crate::remote_ipc::session::{RemoteControlPhase, SessionHandle};
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.update_detached_window_runtime_flags(93009, false, "retirement_test");
    let registration = register(&mut app, &ctx, &[93009])[0];
    let complete = app.sent_composition_save_for_lifecycle_test();
    let session = SessionHandle::new();
    app.set_remote_session_handle(session.clone());
    session.acquire(mimageviewer_ipc::SessionAcquireRequest {
        client_id: "retirement-logic".to_owned(),
        peer: mimageviewer_ipc::SessionPeerInfo {
            connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
            device_name: None,
        },
    });
    let frame = app.frame_counter;
    let sequence = app.input_seq;
    let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
    let (output, _) = backend.update_logic_only(&mut app.app, Default::default());
    assert_eq!(
        session.snapshot().phase,
        RemoteControlPhase::AcquiringRemote
    );
    assert!(app.detached_window_manager.is_empty());
    let mut exact = Vec::new();
    // This headless consumer uses the production Context retirement API; the
    // native resources/renderer adapters are exercised by eframe's same filter.
    for command in output.viewport_commands.values().flatten() {
        if let egui::ViewportCommand::RetireApproved(approval) = command {
            exact.extend_from_slice(approval.registrations());
            ctx.retire_approved_viewports(approval);
        }
    }
    ctx.take_viewport_retirement_texture_delta().clear();
    assert!(exact.contains(&registration));
    assert!(
        ctx.viewport_registration(registration.viewport_id())
            .is_none()
    );
    let (_output, _) = backend.update_logic_only(&mut app.app, Default::default());
    assert_eq!(
        session.snapshot().phase,
        RemoteControlPhase::AcquiringRemote,
        "delivered retirement cannot bypass the existing sent-save owner"
    );
    complete(Ok(()));
    let (_output, _) = backend.update_logic_only(&mut app.app, Default::default());
    assert_eq!(session.snapshot().phase, RemoteControlPhase::RemoteActive);
    assert_eq!(app.frame_counter, frame);
    assert_eq!(app.input_seq, sequence);
}

#[test]
fn viewport_retirement_remote_standalone_fullscreen_keeps_exact_authority_after_context_gc() {
    use crate::remote_ipc::session::{RemoteControlPhase, SessionHandle};
    for gc_before_close in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let id = app.fullscreen_viewport_id();
        ctx.run_ui(Default::default(), |_| {
            ctx.show_viewport_deferred(id, egui::ViewportBuilder::default(), |_, _| {});
        })
        .drop_without_applying_deltas();
        let registration = ctx.viewport_registration(id).unwrap();
        ctx.run_ui(
            egui::RawInput {
                viewport_id: id,
                viewports: [
                    (egui::ViewportId::ROOT, egui::ViewportInfo::default()),
                    (
                        id,
                        egui::ViewportInfo {
                            parent: Some(egui::ViewportId::ROOT),
                            ..Default::default()
                        },
                    ),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            },
            |ui| {
                app.capture_current_detached_viewport_registration(ui.ctx());
            },
        )
        .drop_without_applying_deltas();
        app.fs_viewport_shown = true;
        app.fs_viewport_presentation = Some(ViewerPresentation::Fullscreen);
        if gc_before_close {
            ctx.run_ui(Default::default(), |_| {})
                .drop_without_applying_deltas();
            assert!(ctx.viewport_registration(id).is_none());
        }
        let session = SessionHandle::new();
        app.set_remote_session_handle(session.clone());
        session.acquire(mimageviewer_ipc::SessionAcquireRequest {
            client_id: "retirement-standalone".to_owned(),
            peer: mimageviewer_ipc::SessionPeerInfo {
                connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                device_name: None,
            },
        });
        let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
        let (output, _) = backend.update_logic_only(&mut app.app, Default::default());
        assert_eq!(
            session.snapshot().phase,
            RemoteControlPhase::AcquiringRemote
        );
        assert!(!app.fs_viewport_shown);
        let mut exact = Vec::new();
        for command in output.viewport_commands.values().flatten() {
            if let egui::ViewportCommand::RetireApproved(approval) = command {
                exact.extend_from_slice(approval.registrations());
                ctx.retire_approved_viewports(approval);
            }
        }
        ctx.take_viewport_retirement_texture_delta().clear();
        assert!(exact.contains(&registration));
        let (_output, _) = backend.update_logic_only(&mut app.app, Default::default());
        assert_eq!(session.snapshot().phase, RemoteControlPhase::RemoteActive);
    }
}

#[test]
fn viewport_retirement_remote_full_and_discard_wait_for_outer_delivery() {
    use crate::remote_ipc::session::{RemoteControlPhase, SessionHandle};
    struct FullProbe<'a> {
        app: &'a mut App,
        session: SessionHandle,
        discard: bool,
        passes: usize,
        phases: Vec<RemoteControlPhase>,
    }
    impl eframe::App for FullProbe<'_> {
        fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            eframe::App::logic(self.app, ctx, frame);
            self.phases.push(self.session.snapshot().phase);
        }
        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
            self.passes += 1;
            assert_eq!(
                self.session.snapshot().phase,
                RemoteControlPhase::AcquiringRemote
            );
            assert!(ui.ctx().has_pending_viewport_retirements());
            if self.discard && self.passes == 1 {
                ui.ctx()
                    .request_discard("retirement actual Remote second pass");
            }
        }
    }
    for discard in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        app.update_detached_window_runtime_flags(93010, false, "retirement_test");
        let registration = register(&mut app, &ctx, &[93010])[0];
        let session = SessionHandle::new();
        app.set_remote_session_handle(session.clone());
        session.acquire(mimageviewer_ipc::SessionAcquireRequest {
            client_id: "retirement-full".to_owned(),
            peer: mimageviewer_ipc::SessionPeerInfo {
                connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                device_name: None,
            },
        });
        let mut backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
        let mut probe = FullProbe {
            app: &mut app.app,
            session: session.clone(),
            discard,
            passes: 0,
            phases: Vec::new(),
        };
        let output = backend.update(&mut probe, Default::default());
        assert_eq!(probe.passes, if discard { 2 } else { 1 });
        assert!(
            probe
                .phases
                .iter()
                .all(|phase| *phase == RemoteControlPhase::AcquiringRemote)
        );
        assert!(probe.app.detached_window_manager.is_empty());
        let transferred = approvals(&output);
        for command in output
            .viewport_output
            .values()
            .flat_map(|viewport| &viewport.commands)
        {
            if let egui::ViewportCommand::RetireApproved(approval) = command {
                ctx.retire_approved_viewports(approval);
            }
        }
        output.drop_without_applying_deltas();
        ctx.take_viewport_retirement_texture_delta().clear();
        assert_eq!(
            transferred
                .iter()
                .filter(|&&value| value == registration)
                .count(),
            1
        );
        let (_output, _) = backend.update_logic_only(probe.app, Default::default());
        assert_eq!(session.snapshot().phase, RemoteControlPhase::RemoteActive);
    }
}
