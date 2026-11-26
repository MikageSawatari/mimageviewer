//! Video grid-open regressions use native input after the real open/initialization.
use super::*;
use crate::ui_fullscreen::{FullscreenPrimarySuppression, FullscreenPrimarySuppressionReason};
use crate::video::native_window::{
    NativeVideoMouseButton, NativeVideoMouseButtonEvent, NativeVideoMouseInputOwner,
    NativeVideoWindowEvent, NativeVideoWindowSource,
};

fn player(app: &App) -> &crate::video::VideoPlayer {
    match app.fs_cache.get(&0) {
        Some(FsCacheEntry::Video { player, .. }) => player,
        _ => panic!("ready video fixture missing"),
    }
}

fn native_left(app: &mut App, ctx: &egui::Context, down: bool, double_click: bool, receipt: u64) {
    let source_epoch = player(app).native_source_epoch().unwrap();
    let owner = NativeVideoMouseInputOwner {
        window_source: NativeVideoWindowSource::Presenter,
        receiver_hwnd: player(app).native_presenter_hwnd(),
        // Same initial-generation witness as native_video's mouse hold tests.
        window_generation: 1,
    };
    app.handle_native_video_output_event(
        ctx,
        0,
        source_epoch,
        crate::video::NativeVideoOutputEvent::Window(NativeVideoWindowEvent::MouseButton(
            NativeVideoMouseButtonEvent {
                receipt: crate::mouse_seek_debug::test_receipt(receipt),
                owner,
                button: NativeVideoMouseButton::Left,
                down,
                double_click,
                x: 300,
                y: 180,
                shift: false,
                ctrl: false,
                #[cfg(feature = "test-script")]
                smoke_metadata: None,
            },
        )),
    );
}

fn grid_video_open(stacked: bool) {
    let _input = crate::key_input::lock_test_input();
    let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let folder = app.tmp.path().join("video-open-906");
    std::fs::create_dir(&folder).unwrap();
    let path = folder.join("movie.mp4");
    app.settings.detached_viewer_enabled = false;
    app.settings.vst3_enabled = false;
    app.settings.vst3_gui_visible = false;
    app.current_folder = Some(folder.clone());
    if stacked {
        let view = crate::filename_stack::StackView::build(
            folder,
            Vec::new(),
            Vec::new(),
            vec![crate::filename_stack::StackMember {
                path: path.clone(),
                mtime: 0,
                size: Some(1),
                sort_meta: crate::settings::ListingSortMetadata::new(0, Some(1)),
                is_video: true,
            }],
            '_',
            crate::settings::SortOrder::FileName,
        );
        let (items, metas) = view.materialize_aggregated();
        app.items = items;
        app.image_metas = metas;
        app.stack_view = Some(Arc::new(view));
        app.stack_mode_requested = true;
    } else {
        app.items = vec![GridItem::Video(path.clone())];
        app.image_metas = vec![None];
    }
    app.thumbnails = vec![ThumbnailState::Pending];
    app.rebuild_visible_indices();

    // Hold only decoder creation at the existing startup gate. The real grid
    // handler, stack worker adoption, open_fullscreen and its resets all run.
    // No HWND, audio device, decoder, or real user's profile is created here.
    let (migration, _held_completion) =
        crate::effetune::startup::StartupMigration::pending_for_test();
    app.audio_plugin_migration = migration;
    assert!(app.media_startup_load_pending());
    assert!(crate::ui_main::ratingnav649_pointer_open_for_test(&mut app, 0, false).is_none());
    if stacked {
        // The asynchronous grid entry arms on the LIST viewport's release.
        assert_eq!(
            app.fs_primary_suppression,
            FullscreenPrimarySuppression::ReleasedThisPass(
                FullscreenPrimarySuppressionReason::Other
            )
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.stack_showing_flat {
            assert!(
                std::time::Instant::now() < deadline,
                "flat open did not finish"
            );
            app.poll_stack_script(&ctx);
            std::thread::yield_now();
        }
    }
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.items, vec![GridItem::Video(path.clone())]);
    assert_eq!(app.vst3_deferred_media_open, Some(0));
    assert_eq!(
        app.fs_primary_suppression,
        FullscreenPrimarySuppression::Idle
    );
    assert!(app.fs_focus_regained_at.is_none());

    // Supply readiness at the decoder boundary, after the actual open has
    // initialized everything. NativeVideoOutput is disconnected: no live UI.
    app.audio_plugin_migration = Default::default();
    app.vst3_deferred_media_open = None;
    let ready = crate::video::VideoPlayer::stream_ready_disconnected_for_test(path);
    ready.set_playing(true);
    app.fs_cache.insert(
        0,
        FsCacheEntry::Video {
            player: Box::new(ready),
            load_seq: 1,
        },
    );
    assert!(player(&app).intent_playing());

    // Normal Windows video uses the presenter queue, not the egui canvas.
    // Exercise that production ingress: the opening double-click's second
    // down is WM_LBUTTONDBLCLK, and its up must not toggle playback. This also
    // covers a late delivery; a list release alone has no native press owner.
    native_left(&mut app, &ctx, true, true, 90601);
    native_left(&mut app, &ctx, false, false, 90602);
    assert!(player(&app).intent_playing());
    native_left(&mut app, &ctx, false, false, 90603);
    assert!(player(&app).intent_playing());

    // The very first new presenter click must work without a sacrificial click.
    native_left(&mut app, &ctx, true, false, 90604);
    assert!(player(&app).intent_playing());
    native_left(&mut app, &ctx, false, false, 90605);
    assert!(!player(&app).intent_playing());
}

#[test]
fn suppression_906_main_grid_video_open_does_not_toggle_second_click() {
    grid_video_open(false);
}

#[test]
fn suppression_906_stack_grid_video_open_does_not_toggle_second_click() {
    grid_video_open(true);
}
