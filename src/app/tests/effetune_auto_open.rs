//! Handler checks use disposable AppTestEnv and disconnected playback, never a native UI.
use super::*;
use crate::effetune::gui_gate::{AutoSuppression, GuiGate};
use crate::effetune::{EffetuneRuntime, LoadOrigin, ShowIntent};
use crate::video::PlaybackStartOrigin;

fn prepare(app: &mut App) -> std::sync::Arc<GuiGate> {
    app.settings.effetune_auto_open_on_video = true;
    app.publish_effetune_auto_setting();
    app.effetune.runtime = EffetuneRuntime::Loading {
        origin: LoadOrigin::Startup,
        open_gui_when_ready: None,
    };
    let gate = app.effetune.gui_gate().unwrap();
    gate.set_auto_factor(AutoSuppression::RootHidden, false);
    gate.set_auto_factor(AutoSuppression::Minimized, false);
    gate.set_auto_factor(AutoSuppression::RemoteBlocked, false);
    app.viewer_presentation = ViewerPresentation::MainWindow;
    gate
}

fn successful_player(app: &mut App, path: PathBuf) -> crate::video::PlaybackSuccess {
    app.items.push(GridItem::Video(path.clone()));
    app.fullscreen_idx = Some(app.items.len() - 1);
    app.publish_effetune_auto_fullscreen();
    let player = crate::video::VideoPlayer::disconnected_for_test(path, 20.0);
    player.bind_playback_viewer_context(app.projected_viewer_context_id().serial());
    player.set_auto_presentation_reader(app.effetune.auto_reader().unwrap());
    player.confirm_playback_start_for_test(PlaybackStartOrigin::NewSource);
    let success = player.pending_playback_success_for_test().unwrap();
    app.fs_cache.insert(
        app.items.len() - 1,
        FsCacheEntry::Video {
            player: Box::new(player),
            load_seq: 0,
        },
    );
    success
}

fn requested(app: &App) -> bool {
    matches!(
        app.effetune.runtime,
        EffetuneRuntime::Loading {
            open_gui_when_ready: Some(ShowIntent::AutoVideo(_)),
            ..
        }
    )
}

#[test]
fn effetune_auto_open_poll_accepts_adopted_success_once() {
    let mut app = setup_app_for_test();
    let gate = prepare(&mut app);
    let path = app.tmp.path().join("eligible.mp4");
    successful_player(&mut app, path);
    assert!(gate.auto_snapshot().allowed);
    app.poll_video(&egui::Context::default());
    assert!(requested(&app));
    if let Some(FsCacheEntry::Video { player, .. }) = app.fs_cache.get(&0) {
        assert!(player.take_playback_success().is_none());
    }
}

#[test]
fn effetune_auto_open_poll_rejects_suppression_round_trips_without_spending() {
    for factor in [
        AutoSuppression::SettingOff,
        AutoSuppression::RootHidden,
        AutoSuppression::Fullscreen,
        AutoSuppression::Minimized,
        AutoSuppression::RemoteBlocked,
    ] {
        let mut app = setup_app_for_test();
        let gate = prepare(&mut app);
        let path = app.tmp.path().join("roundtrip.mp4");
        successful_player(&mut app, path.clone());
        gate.set_auto_factor(factor, true);
        gate.set_auto_factor(factor, false);
        app.poll_video(&egui::Context::default());
        assert!(!requested(&app), "{factor:?}");
        let player = match app.fs_cache.get(&0).unwrap() {
            FsCacheEntry::Video { player, .. } => player,
            _ => unreachable!(),
        };
        assert!(player.take_playback_success().is_none());
        player.set_playing_with_origin(false, PlaybackStartOrigin::UserPlay);
        player.confirm_playback_start_for_test(PlaybackStartOrigin::UserPlay);
        app.poll_video(&egui::Context::default());
        assert!(
            requested(&app),
            "a discarded stale success must leave Armed"
        );
    }
}

#[test]
fn effetune_auto_open_poll_off_drains_and_on_normalize_is_internal() {
    let mut app = setup_app_for_test();
    prepare(&mut app);
    app.settings.effetune_auto_open_on_video = false;
    app.publish_effetune_auto_setting();
    let path = app.tmp.path().join("normalize.mp4");
    successful_player(&mut app, path);
    app.poll_video(&egui::Context::default());
    assert!(!requested(&app));
    app.settings.effetune_auto_open_on_video = true;
    app.publish_effetune_auto_setting();
    let player = match app.fs_cache.get(&0).unwrap() {
        FsCacheEntry::Video { player, .. } => player,
        _ => unreachable!(),
    };
    player.set_playing_internal(false, crate::video::InternalContinuation::Normalize);
    player.set_playing_internal(true, crate::video::InternalContinuation::Normalize);
    app.poll_video(&egui::Context::default());
    assert!(!requested(&app));
}

#[test]
fn effetune_auto_open_poll_audio_mode_and_unadopted_cache_are_drained() {
    let mut app = setup_app_for_test();
    prepare(&mut app);
    let path = app.tmp.path().join("excluded.mp4");
    successful_player(&mut app, path);
    app.video_audio_mode = Some(0);
    app.poll_video(&egui::Context::default());
    assert!(!requested(&app));
    app.video_audio_mode = None;
    app.poll_video(&egui::Context::default());
    assert!(!requested(&app));
}

#[test]
fn effetune_auto_fullscreen_projection_retains_sibling_across_context_mounts() {
    let mut app = setup_app_for_test();
    let gate = prepare(&mut app);
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.publish_effetune_auto_fullscreen();
    assert!(!gate.auto_snapshot().allowed);
    let fullscreen_revision = gate.auto_snapshot().revision;
    app.build_active_context_for_test(Some(778), DetachedSource::Image, |viewer| {
        viewer.viewer_presentation = ViewerPresentation::DetachedWindow;
        viewer.fullscreen_idx = Some(0);
    });
    app.publish_effetune_auto_fullscreen();
    assert_eq!(gate.auto_snapshot().revision, fullscreen_revision);
    assert!(
        !gate.auto_snapshot().allowed,
        "main sibling is still fullscreen"
    );
}

// These regressions enter through App's real success/load polling and the real
// host-control worker, DspBridge attach and checked visibility ACK methods.
fn cancellation_delivery(stage: crate::effetune::delivery_tests::DeliveryStage) {
    use crate::effetune::delivery_tests::{DeliveryStage, FakeHost};
    use crate::video::dsp::bridge::GuiVisibilityOutcome;
    for factor in [
        AutoSuppression::SettingOff,
        AutoSuppression::RootHidden,
        AutoSuppression::Fullscreen,
        AutoSuppression::Minimized,
        AutoSuppression::RemoteBlocked,
    ] {
        for round_trip_before_delivery in [false, true] {
            let mut app = setup_app_for_test();
            let gate = prepare(&mut app);
            let host = FakeHost::new(&app.effetune, stage);
            let ctx = egui::Context::default();
            let path = app.tmp.path().join("delivery.mp4");
            let first_start = successful_player(&mut app, path);
            let success = gate.auto_snapshot();
            app.poll_video(&ctx);
            assert!(requested(&app));
            let label = format!("{factor:?}/{stage:?}/round_trip={round_trip_before_delivery}");
            if stage != DeliveryStage::Loading {
                app.effetune
                    .set_test_load_bridge_completion(Arc::clone(&host.bridge));
                app.poll_effetune(&ctx);
                host.wait_stage(stage);
            }
            gate.set_auto_factor(factor, true);
            if round_trip_before_delivery {
                gate.set_auto_factor(factor, false);
            }
            if stage == DeliveryStage::Loading {
                app.effetune
                    .set_test_load_bridge_completion(Arc::clone(&host.bridge));
                app.poll_effetune(&ctx);
            } else {
                host.release_stage();
            }
            let cancelled = host.fence(&app.effetune);
            assert!(
                !cancelled.requested && !cancelled.visible,
                "{label}: {cancelled:?}"
            );
            assert_eq!(cancelled.shows, 0, "{label}");
            assert_eq!(
                (cancelled.activate, cancelled.auto_activate),
                (0, 0),
                "{label}"
            );
            assert_eq!(
                cancelled.attached,
                usize::from(stage != DeliveryStage::Loading),
                "{label}"
            );
            assert_eq!(
                cancelled.delivered,
                usize::from(stage == DeliveryStage::HostDelivery),
                "{label}"
            );
            if stage == DeliveryStage::HostDelivery {
                assert_eq!(cancelled.auto_revision, Some(success.revision), "{label}");
                assert_eq!(
                    cancelled.outcome,
                    Some(GuiVisibilityOutcome::Cancelled),
                    "{label}"
                );
            }
            gate.set_auto_factor(factor, false);
            let recovered = host.reconcile(&app.effetune);
            app.poll_effetune(&ctx); // Drain the real ordered host visibility signal stream.
            assert!(
                !recovered.requested && !recovered.visible,
                "{label}: {recovered:?}"
            );
            assert_eq!(
                (recovered.shows, recovered.activate, recovered.auto_activate),
                (0, 0, 0),
                "{label}"
            );
            assert!(!host.bridge.slots()[0].gui_visible, "{label}");
            // A later eligible user start must not resurrect the spent Auto request.
            let FsCacheEntry::Video { player, .. } = app.fs_cache.get(&0).unwrap() else {
                unreachable!()
            };
            player.set_playing_with_origin(false, PlaybackStartOrigin::UserPlay);
            player.confirm_playback_start_for_test(PlaybackStartOrigin::UserPlay);
            let next_start = player
                .pending_playback_success_for_test()
                .expect("pause -> play must generate a real unconsumed success");
            assert_ne!(next_start.id, first_start.id, "{label}");
            assert_eq!(next_start.origin, PlaybackStartOrigin::UserPlay, "{label}");
            assert!(next_start.projection.allowed, "{label}");
            assert_eq!(
                next_start.viewer_context, first_start.viewer_context,
                "{label}"
            );
            app.poll_video(&ctx);
            let FsCacheEntry::Video { player, .. } = app.fs_cache.get(&0).unwrap() else {
                unreachable!()
            };
            assert!(player.playback_success_is_current(next_start.id), "{label}");
            assert!(
                player.pending_playback_success_for_test().is_none(),
                "{label}"
            );
            app.poll_effetune(&ctx);
            let later = host.reconcile(&app.effetune);
            assert_eq!(later.delivered, cancelled.delivered, "{label}");
            assert!(!later.requested && !later.visible, "{label}: {later:?}");
            assert_eq!(
                (later.shows, later.activate, later.auto_activate),
                (0, 0, 0),
                "{label}"
            );
            assert!(
                matches!(app.effetune.runtime, EffetuneRuntime::Running { .. }),
                "{label}"
            );
        }
    }
}

#[test]
fn effetune_auto_open_delivery_cancel_during_loading_has_no_late_popup() {
    cancellation_delivery(crate::effetune::delivery_tests::DeliveryStage::Loading);
}
#[test]
fn effetune_auto_open_delivery_cancel_during_hidden_attach_has_no_late_popup() {
    cancellation_delivery(crate::effetune::delivery_tests::DeliveryStage::HiddenAttach);
}
#[test]
fn effetune_auto_open_delivery_cancel_at_host_has_no_late_popup() {
    cancellation_delivery(crate::effetune::delivery_tests::DeliveryStage::HostDelivery);
}

#[test]
fn effetune_auto_open_delivery_auto_shows_once_without_activate() {
    use crate::effetune::delivery_tests::{DeliveryStage, FakeHost};
    let mut app = setup_app_for_test();
    let gate = prepare(&mut app);
    let host = FakeHost::new(&app.effetune, DeliveryStage::Loading);
    let ctx = egui::Context::default();
    let path = app.tmp.path().join("positive-auto.mp4");
    successful_player(&mut app, path);
    let success = gate.auto_snapshot();
    app.poll_video(&ctx);
    app.effetune
        .set_test_load_bridge_completion(Arc::clone(&host.bridge));
    app.poll_effetune(&ctx);
    let shown = host.fence(&app.effetune);
    assert!(shown.requested && shown.visible);
    assert_eq!((shown.attached, shown.delivered, shown.shows), (1, 1, 1));
    assert_eq!((shown.activate, shown.auto_activate), (0, 0));
    assert_eq!(shown.auto_revision, Some(success.revision));
    app.poll_effetune(&ctx);
    assert!(host.bridge.slots()[0].gui_visible);
    app.poll_video(&ctx);
    app.poll_effetune(&ctx);
    let later = host.reconcile(&app.effetune);
    assert_eq!(
        (
            later.delivered,
            later.shows,
            later.activate,
            later.auto_activate
        ),
        (1, 1, 0, 0)
    );
}

#[test]
fn effetune_auto_open_delivery_manual_control_activates_and_publishes_visible() {
    use crate::effetune::delivery_tests::{DeliveryStage, FakeHost};
    let mut app = setup_app_for_test();
    prepare(&mut app);
    let host = FakeHost::new(&app.effetune, DeliveryStage::Loading);
    let ctx = egui::Context::default();
    app.effetune.click_idle(None, None);
    app.effetune
        .set_test_load_bridge_completion(Arc::clone(&host.bridge));
    app.poll_effetune(&ctx);
    let shown = host.fence(&app.effetune);
    assert!(shown.requested && shown.visible);
    assert_eq!(
        (shown.attached, shown.delivered, shown.shows, shown.activate),
        (1, 1, 1, 1)
    );
    assert_eq!(shown.auto_activate, 0);
    assert_eq!(shown.auto_revision, None);
    app.poll_effetune(&ctx);
    assert!(host.bridge.slots()[0].gui_visible);
}
