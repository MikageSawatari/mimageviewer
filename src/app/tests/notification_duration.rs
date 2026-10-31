use super::{phase_c_support::setup_app, *};
use crate::settings::NotificationDuration;

#[test]
fn notification_duration_publisher_preserves_surface_reveal_and_replaces_previous_toast() {
    let mut app = setup_app();
    app.settings.notification_duration = NotificationDuration::Short;
    app.show_feedback_toast("成功".into());
    assert!((app.fs_feedback_toast.as_ref().unwrap().2 - 0.72).abs() < 0.00001);
    app.show_feedback_warning("長".repeat(80));
    assert_eq!(app.fs_feedback_toast.as_ref().unwrap().2, 5.5);
    app.show_feedback_toast_on_with_duration("面指定".into(), ActionSurface::MainWindow, 4.0);
    assert_eq!(app.fs_feedback_toast.as_ref().unwrap().2, 2.4);
    assert_eq!(
        app.fs_feedback_toast_surface,
        Some(ActionSurface::MainWindow)
    );
    let path = app.tmp.path().join("日本語の保存画像.png");
    app.show_capture_saved_toast(path.clone());
    assert_eq!(
        app.fs_feedback_toast_action
            .as_ref()
            .and_then(crate::feedback::FeedbackAction::reveal_path),
        Some(&path)
    );
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        "保存しました: 日本語の保存画像.png"
    );
    app.settings.notification_duration = NotificationDuration::Long;
    app.show_feedback_warning_with_duration("重要な案内".into(), 5.0);
    assert_eq!(app.fs_feedback_toast.as_ref().unwrap().2, 7.5);
    assert!(app.fs_feedback_toast_action.is_none());
}

#[cfg(windows)]
#[test]
fn notification_duration_native_transport_applies_policy_once() {
    let (mut app, probe, idx) = native_notification_app();

    for (setting, factor) in [
        (NotificationDuration::Short, 0.6),
        (NotificationDuration::Standard, 1.0),
        (NotificationDuration::Long, 1.5),
    ] {
        app.settings.notification_duration = setting;
        assert_eq!(
            app.native_chrome_snapshot(idx).policy.notification_duration,
            setting
        );
        app.show_native_video_overlay_toast("native 操作".into(), false);
        assert_eq!(
            probe.drain_toasts()[0].1,
            Some(std::time::Duration::from_secs_f32(1.8 * factor))
        );
        app.show_native_video_overlay_warning("native 警告".into(), false);
        let warning_factor = if setting == NotificationDuration::Long {
            1.5
        } else {
            1.0
        };
        assert_eq!(
            probe.drain_toasts()[0].1,
            Some(std::time::Duration::from_secs_f32(1.8 * warning_factor))
        );
        app.show_feedback_toast("成功".into());
        let egui_seconds = app.fs_feedback_toast.as_ref().unwrap().2;
        let toasts = probe.drain_toasts();
        assert_eq!(toasts.len(), 1);
        assert_eq!(
            toasts[0].1,
            Some(std::time::Duration::from_secs_f32(egui_seconds))
        );
        assert!((egui_seconds - 1.2 * factor).abs() < 0.00001);

        app.show_feedback_toast_on_with_duration("面指定".into(), ActionSurface::Viewer, 4.0);
        assert_eq!(
            probe.drain_toasts()[0].1,
            Some(std::time::Duration::from_secs_f32(4.0 * factor))
        );
        app.show_feedback_warning_on_with_duration("警告".into(), ActionSurface::Viewer, 5.0);
        let warning_factor = if setting == NotificationDuration::Long {
            1.5
        } else {
            1.0
        };
        assert_eq!(
            probe.drain_toasts()[0].1,
            Some(std::time::Duration::from_secs_f32(5.0 * warning_factor))
        );
        app.show_feedback_toast_on("一覧のみ".into(), ActionSurface::MainWindow);
        assert!(probe.drain_toasts().is_empty());
    }
}

#[cfg(windows)]
fn native_notification_app() -> (
    phase_c_support::AppTestEnv,
    crate::video::NativeUiProbeForTest,
    usize,
) {
    let mut app = setup_app();
    let idx = app.items.len();
    let path = PathBuf::from(r"C:\clips\notification.mp4");
    app.items.push(GridItem::Video(path.clone()));
    app.thumbnails.push(ThumbnailState::Pending);
    let mut player = crate::video::VideoPlayer::stream_ready_disconnected_for_test(path);
    let probe = player.install_native_ui_probe_for_test();
    app.fs_cache.insert(
        idx,
        FsCacheEntry::Video {
            player: Box::new(player),
            load_seq: 0,
        },
    );
    app.fullscreen_idx = Some(idx);
    app.viewer_presentation = ViewerPresentation::Fullscreen;

    (app, probe, idx)
}

#[cfg(windows)]
#[test]
fn notification_duration_audio_failure_issued_by_poll_preserves_native_default_and_music_text_time()
{
    for music in [false, true] {
        let (mut app, probe, idx) = native_notification_app();
        if let Some(FsCacheEntry::Video { player, .. }) = app.fs_cache.get_mut(&idx) {
            player.set_opened_audio_stream_for_test(2, 1);
            player.set_opened_audio_stream_for_test(1, 1);
        }
        app.video_audio_mode = music.then_some(idx);
        for (setting, factor) in [
            (NotificationDuration::Short, 1.0),
            (NotificationDuration::Standard, 1.0),
            (NotificationDuration::Long, 1.5),
        ] {
            app.settings.notification_duration = setting;
            for saved_open_notice in [false, true] {
                if saved_open_notice {
                    let FsCacheEntry::Video { player, .. } = app.fs_cache.get(&idx).unwrap() else {
                        unreachable!()
                    };
                    player.set_saved_audio_track_open_notice_for_test();
                } else {
                    app.select_video_audio_track(idx, 2);
                    let FsCacheEntry::Video { player, .. } = app.fs_cache.get(&idx).unwrap() else {
                        unreachable!()
                    };
                    player.fail_desired_audio_track_for_test();
                }
                app.poll_audio_track_notice_toast(idx);
                let toasts = probe.drain_toast_timings();
                assert_eq!(toasts.len(), 1);
                let (text, timing) = &toasts[0];
                assert_eq!(
                    text,
                    if saved_open_notice {
                        "保存した音声トラックを開けなかったため、既定のトラックで再生します"
                    } else {
                        "音声トラックを切り替えられませんでした"
                    }
                );
                let timing = timing.unwrap();
                let base = if music {
                    crate::feedback::feedback_toast_duration(text)
                } else {
                    1.8
                };
                assert_eq!(
                    timing.native_linger(),
                    std::time::Duration::from_secs_f32(base * factor)
                );
                if music {
                    assert_eq!(app.fs_feedback_toast.as_ref().unwrap().2, base * factor);
                } else {
                    assert!(app.fs_feedback_toast.is_none());
                }
                assert!((timing.native_opacity_at(1.625 * factor, false) - 0.5).abs() < 0.00001);
                app.poll_audio_track_notice_toast(idx);
                assert!(
                    probe.drain_toasts().is_empty(),
                    "notification consumed once"
                );
            }
        }
    }
}

#[cfg(windows)]
#[test]
fn notification_duration_capture_and_debug_completion_preserve_surface_baselines_and_saved_links() {
    let (mut app, probe, idx) = native_notification_app();
    for (setting, factor) in [
        (NotificationDuration::Short, 0.6),
        (NotificationDuration::Standard, 1.0),
        (NotificationDuration::Long, 1.5),
    ] {
        app.settings.notification_duration = setting;
        let capture = app.tmp.path().join("capture.png");
        let manifest = app.tmp.path().join("debug/manifest.json");
        for debug in [false, true] {
            if debug {
                let (tx, rx) = std::sync::mpsc::channel();
                tx.send(Ok(crate::pipeline_debug::PipelineDebugExportComplete {
                    dir: app.tmp.path().join("debug"),
                    manifest_path: manifest.clone(),
                    stage_count: 1,
                }))
                .unwrap();
                app.pipeline_debug_export_pending =
                    Some(crate::pipeline_debug::PipelineDebugExportPending { rx });
                app.poll_pipeline_debug_export_pending(&egui::Context::default());
            } else {
                app.show_capture_saved_toast(capture.clone());
            }
            assert_eq!(app.fs_feedback_toast.as_ref().unwrap().2, 1.2 * factor);
            assert_eq!(
                app.fs_feedback_toast_action
                    .as_ref()
                    .and_then(crate::feedback::FeedbackAction::reveal_path),
                Some(if debug { &manifest } else { &capture })
            );
            let toasts = probe.drain_toast_timings();
            assert_eq!(toasts.len(), 1);
            let timing = toasts[0].1.unwrap();
            assert_eq!(
                timing.native_linger(),
                std::time::Duration::from_secs_f32(1.8 * factor)
            );
            assert_eq!(timing.native_opacity_at(1.4 * factor, false), 1.0);
            assert!((timing.native_opacity_at(1.625 * factor, false) - 0.5).abs() < 0.00001);
            assert_eq!(timing.native_opacity_at(1.8 * factor, false), 0.0);
        }
        app.native_video_seek_relative_with_hint(idx, -10.0);
        let toasts = probe.drain_toast_timings();
        assert_eq!(toasts.len(), 1);
        assert_eq!(toasts[0].0, "動画先頭です");
        let timing = toasts[0].1.unwrap();
        assert_eq!(
            timing.native_linger(),
            std::time::Duration::from_secs_f32(0.7)
        );
        assert_eq!(timing.native_opacity_at(0.699, true), 1.0);
    }
}
