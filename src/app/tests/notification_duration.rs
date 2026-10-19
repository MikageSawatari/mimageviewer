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
    assert_eq!(app.fs_feedback_toast_reveal_path.as_ref(), Some(&path));
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        "保存しました: 日本語の保存画像.png"
    );
    app.settings.notification_duration = NotificationDuration::Long;
    app.show_feedback_warning_with_duration("重要な案内".into(), 5.0);
    assert_eq!(app.fs_feedback_toast.as_ref().unwrap().2, 7.5);
    assert!(app.fs_feedback_toast_reveal_path.is_none());
}

#[cfg(windows)]
#[test]
fn notification_duration_native_transport_applies_policy_once() {
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
