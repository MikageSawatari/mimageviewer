use super::phase_c_support::setup_app;
use super::*;
use crate::ui_fullscreen::FsPageNav;

#[test]
fn manual_nav_wrap_detached_uses_only_mounted_order_and_keeps_main_unchanged() {
    let mut env = setup_app();
    let app = &mut env.app;
    let ctx = egui::Context::default();
    app.settings.fullscreen_manual_nav_wrap = true;
    app.items = vec![
        GridItem::Image("c:/detached/first.png".into()),
        GridItem::Image("c:/detached/last.png".into()),
    ];
    app.thumbnails = vec![ThumbnailState::Pending; 2];
    app.visible_indices = vec![1, 0];
    app.viewer_navigation_caches.invalidate();
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.detached_viewer_independent_active = true;
    app.set_detached_window_binding_for_test(Some(1400));
    app.stash_mounted_as_active_for_test(1400);
    app.begin_active_detached_session(1400, DetachedSource::Image);

    app.items = vec![GridItem::Image("c:/main/only.png".into())];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.visible_indices = vec![0];
    app.viewer_navigation_caches.invalidate();
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::MainWindow;
    app.with_active_viewer_context(|mounted| {
        assert_eq!(mounted.fullscreen_idx, Some(0));
        let nav = mounted.spread_page_nav(1);
        assert_eq!(nav, FsPageNav::Delta(1));
        mounted.handle_fs_navigation(&ctx, false, false, None, None, None, nav, None, 0);
        assert_eq!(mounted.fullscreen_idx, Some(1));
        assert!(mounted.fs_boundary_hint.is_none());
        assert_eq!(mounted.items.len(), 2);
    })
    .expect("detached context");
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.items.len(), 1);
    assert!(
        matches!(&app.items[0], GridItem::Image(path) if path == std::path::Path::new("c:/main/only.png"))
    );
    assert_eq!(app.visible_indices, vec![0]);
}

#[test]
fn manual_nav_wrap_does_not_replace_ctrl_folder_navigation_with_item_wrap() {
    for wrap in [false, true] {
        for forward in [false, true] {
            let mut env = setup_app();
            let root = env.tmp.path().join("manual-wrap-folders");
            for folder in ["a", "b", "c"] {
                std::fs::create_dir_all(root.join(folder)).unwrap();
                std::fs::write(
                    root.join(folder).join("page.png"),
                    b"navigation-only fixture",
                )
                .unwrap();
            }
            let app = &mut env.app;
            let current = root.join("b");
            app.current_folder = Some(current.clone());
            app.items = vec![GridItem::Image(current.join("page.png"))];
            app.thumbnails = vec![ThumbnailState::Pending];
            app.visible_indices = vec![0];
            app.viewer_navigation_caches.invalidate();
            app.fullscreen_idx = Some(0);
            app.settings.fullscreen_manual_nav_wrap = wrap;
            app.handle_fs_navigation(
                &egui::Context::default(),
                false,
                false,
                Some(if forward { 1 } else { -1 }),
                None,
                None,
                FsPageNav::None,
                None,
                0,
            );
            assert!(app.folder_nav_pending.is_some());
            assert_eq!(app.pending_folder_nav_mode.perf_tag(), "fullscreen");
            assert!(app.fs_boundary_hint.is_none());
            app.cancel_pending_folder_nav();
        }
    }
}
