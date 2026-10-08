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

fn successful_player(app: &mut App, path: PathBuf) {
    app.items.push(GridItem::Video(path.clone()));
    app.fullscreen_idx = Some(app.items.len() - 1);
    app.publish_effetune_auto_fullscreen();
    let player = crate::video::VideoPlayer::disconnected_for_test(path, 20.0);
    player.bind_playback_viewer_context(app.projected_viewer_context_id().serial());
    player.set_auto_presentation_reader(app.effetune.auto_reader().unwrap());
    player.confirm_playback_start_for_test(PlaybackStartOrigin::NewSource);
    app.fs_cache.insert(
        app.items.len() - 1,
        FsCacheEntry::Video {
            player: Box::new(player),
            load_seq: 0,
        },
    );
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
