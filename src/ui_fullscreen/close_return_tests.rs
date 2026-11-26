//! C5 owns the unchanged source view until its parent list is adopted.
use super::*;

fn source_pages(app: &mut App, ctx: &egui::Context, count: usize) {
    app.items = (0..count)
        .map(|idx| GridItem::Image(PathBuf::from(format!("C:/c5/book/{idx}.png"))))
        .collect();
    app.visible_indices = (0..count).collect();
    app.thumbnails = (0..count).map(|_| ThumbnailState::Pending).collect();
    app.image_metas = vec![None; count];
    app.fullscreen_idx = Some(0);
    app.spread_mode = SpreadMode::Single;
    for idx in 0..count {
        ready_page(app, ctx, idx);
    }
}

fn ready_page(app: &mut App, ctx: &egui::Context, idx: usize) {
    let tex = ctx.load_texture(
        format!("c5-page-{idx}"),
        egui::ColorImage::filled([4, 8], egui::Color32::WHITE),
        egui::TextureOptions::LINEAR,
    );
    app.thumbnails[idx] = ThumbnailState::loaded_raster(
        tex,
        crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
        false,
        8,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((4, 8)),
        None,
    );
}

#[test]
fn close_return_captures_visible_previous_before_true_close_cleanup() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    source_pages(&mut app, &ctx, 2);
    app.capture_fs_nav_holdover(0);
    let old_id = app
        .fs_holdover_tex
        .as_ref()
        .unwrap()
        .folder_navigation_display_unit()
        .unwrap()
        .pages[0]
        .texture
        .id();
    app.fullscreen_idx = Some(1);
    app.thumbnails[1] = ThumbnailState::Pending;

    let captured = app.capture_fullscreen_close_return_presentation().unwrap();
    app.finish_fullscreen_navigation_for_true_close();
    assert!(app.fs_holdover_tex.is_none());
    app.begin_close_return_navigation_sequence(&captured);
    let previous = app
        .fs_nav_holdover_for_draw()
        .expect("the old painted page survives cleanup");
    assert_eq!(previous.pages[0].texture.id(), old_id);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.fs_nav_is_locked());
    assert!(app.fs_navigation_sequence_blocks_new_target());
}

#[test]
fn close_return_live_render_keeps_fallback_and_input_lock_until_terminal() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    source_pages(&mut app, &ctx, 1);
    let captured = app.capture_fullscreen_close_return_presentation().unwrap();
    app.finish_fullscreen_navigation_for_true_close();
    app.begin_close_return_navigation_sequence(&captured);
    app.fs_zoom = 2.0;
    app.fs_pan = egui::vec2(24.0, -12.0);
    app.fs_free_rotation = 0.4;
    assert!(
        app.fs_nav_holdover_for_draw().is_none(),
        "ready source uses live transforms"
    );
    app.poll_fs_nav_lock(&ctx);
    assert!(app.fs_nav_is_locked());
    assert!(app.fs_navigation_sequence_blocks_new_target());
    assert!(!app.bind_fs_navigation_sequence_to_current_target());

    app.thumbnails[0] = ThumbnailState::Pending;
    assert!(
        app.fs_nav_holdover_for_draw().is_some(),
        "readiness never discards the captured fallback"
    );
    ready_page(&mut app, &ctx, 0);
    assert!(app.fs_nav_holdover_for_draw().is_none());
    assert!(app.fs_navigation_sequence_blocks_new_target());
    app.finish_close_return_navigation_sequence(
        captured.request_id,
        crate::app::FsNavigationSequenceFinish::Superseded,
    );
    assert!(!app.fs_nav_is_locked());
    assert!(!app.fs_navigation_sequence_blocks_new_target());
    assert_eq!(app.fullscreen_idx, Some(0));
}

#[test]
fn close_return_waits_for_every_page_in_current_spread() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    source_pages(&mut app, &ctx, 2);
    app.spread_mode = SpreadMode::Ltr;
    let composition = app.spread_display_composition_for_anchor(0);
    assert_eq!(
        composition.pages.len(),
        2,
        "fixture must have a whole two-page spread"
    );
    let captured = app.capture_fullscreen_close_return_presentation().unwrap();
    app.finish_fullscreen_navigation_for_true_close();
    app.begin_close_return_navigation_sequence(&captured);
    app.thumbnails[1] = ThumbnailState::Pending;
    assert!(app.fs_nav_holdover_for_draw().is_some());
    ready_page(&mut app, &ctx, 1);
    assert!(app.fs_nav_holdover_for_draw().is_none());
    assert!(app.fs_navigation_sequence_blocks_new_target());
}

#[test]
fn close_return_terminal_only_releases_its_own_request_and_exit_resets_panel() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    source_pages(&mut app, &ctx, 1);
    let old = app.capture_fullscreen_close_return_presentation().unwrap();
    let current = app.capture_fullscreen_close_return_presentation().unwrap();
    app.begin_close_return_navigation_sequence(&current);
    app.fs_info_panel.locked = true;
    app.finish_close_return_navigation_sequence(
        old.request_id,
        crate::app::FsNavigationSequenceFinish::Superseded,
    );
    assert!(app.fs_nav_is_locked());
    assert!(app.fs_info_panel.locked);
    app.finish_close_return_navigation_sequence(
        current.request_id,
        crate::app::FsNavigationSequenceFinish::ViewerExited,
    );
    assert!(!app.fs_nav_is_locked());
    assert!(!app.fs_info_panel.locked);
}

#[test]
fn close_return_does_not_change_ctrl_folder_navigation_draw_policy() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    source_pages(&mut app, &ctx, 1);
    app.begin_fs_folder_navigation_sequence(&ctx, 0);
    assert!(
        app.fs_nav_holdover_for_draw().is_some(),
        "Ctrl folder navigation still freezes its source"
    );
    assert!(app.fs_navigation_sequence_blocks_new_target());
}

#[test]
fn close_return_page_and_jump_handlers_discard_new_targets_without_replay() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    source_pages(&mut app, &ctx, 3);
    let captured = app.capture_fullscreen_close_return_presentation().unwrap();
    app.begin_close_return_navigation_sequence(&captured);
    for (page_nav, jump) in [(FsPageNav::Delta(1), None), (FsPageNav::None, Some(2))] {
        app.handle_fs_navigation_with_operation(
            &ctx, false, false, None, None, None, page_nav, jump, 0, None,
        );
        assert_eq!(app.fullscreen_idx, Some(0));
        assert!(app.fs_navigation_sequence_blocks_new_target());
    }
    app.finish_close_return_navigation_sequence(
        captured.request_id,
        crate::app::FsNavigationSequenceFinish::Superseded,
    );
    app.handle_fs_navigation_with_operation(
        &ctx,
        false,
        false,
        None,
        None,
        None,
        FsPageNav::None,
        None,
        0,
        None,
    );
    assert_eq!(
        app.fullscreen_idx,
        Some(0),
        "releasing the wait does not replay a target"
    );
}
