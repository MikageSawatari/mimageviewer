#![cfg(windows)]

use super::handler_tests::{app as scroll_app, frame, paint, seed};
use super::*;

fn animated_app(ctx: &egui::Context) -> crate::app::AppTestEnvForTest {
    let mut app = scroll_app(ctx, SpreadMode::Single);
    app.settings.fullscreen_smooth_page_scroll = true;
    seed(ctx, &mut app);
    frame(ctx, vec![], |_, ctx| {
        app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageScrollDown);
    });
    let animation = app
        .fs_page_scroll_animation
        .as_ref()
        .expect("page-scroll input creates an animation");
    assert_ne!(animation.from, animation.to);
    assert_eq!(app.fs_pan, animation.to);
    assert_eq!(animation.start_time, None);
    assert_eq!(animation.last_painted_pan, animation.from);
    assert_eq!(animation.last_painted_time, None);
    app
}

fn assert_animation(app: &App, expected: &FsPageScrollAnimation) {
    let actual = app
        .fs_page_scroll_animation
        .as_ref()
        .expect("the owning context retains its animation");
    assert_eq!(actual.from, expected.from);
    assert_eq!(actual.to, expected.to);
    assert_eq!(actual.start_time, expected.start_time);
    assert_eq!(actual.last_painted_pan, expected.last_painted_pan);
    assert_eq!(actual.last_painted_time, expected.last_painted_time);
    assert_eq!(actual.origin, expected.origin);
    assert_eq!(actual.viewport, expected.viewport);
    assert_eq!(app.fs_pan, expected.to);
}

#[test]
fn page_scroll_animation_context_roundtrip_preserves_both_owners() {
    let ctx = egui::Context::default();
    let mut app = animated_app(&ctx);
    let main_animation = app.fs_page_scroll_animation.clone().unwrap();
    let mut child_animation = main_animation.clone();
    child_animation.from = Vec2::new(11.0, 19.0);
    child_animation.to = Vec2::new(11.0, -73.0);
    child_animation.start_time = Some(0.031);
    child_animation.last_painted_pan = Vec2::new(11.0, 3.0);
    child_animation.last_painted_time = Some(0.04);
    child_animation.origin.items_generation += 17;
    child_animation.viewport = egui::ViewportId::from_hash_of("context-scroll-child");
    let child = app.build_window_context_for_test(1823001, |owner| {
        assert!(owner.fs_page_scroll_animation.is_none());
        owner.items_generation = child_animation.origin.items_generation;
        owner.fs_pan = child_animation.to;
        owner.fs_page_scroll_animation = Some(child_animation.clone());
    });
    assert_animation(&app, &main_animation);

    app.with_window_context_for_test(child, |owner| {
        assert_animation(owner, &child_animation);
        child_animation.to.y -= 37.0;
        child_animation.start_time = Some(0.05);
        child_animation.last_painted_pan.y -= 13.0;
        child_animation.last_painted_time = Some(0.06);
        owner.fs_pan = child_animation.to;
        owner.fs_page_scroll_animation = Some(child_animation.clone());
    });
    assert_animation(&app, &main_animation);
    app.with_window_context_for_test(child, |owner| {
        assert_animation(owner, &child_animation);
        owner.fs_page_scroll_animation = None;
    });
    assert_animation(&app, &main_animation);
    app.with_window_context_for_test(child, |owner| {
        assert!(owner.fs_page_scroll_animation.is_none());
        assert_eq!(owner.fs_pan, child_animation.to);
    });
    assert_animation(&app, &main_animation);
}

#[test]
fn sibling_temporary_mount_does_not_retire_the_active_scroll_animation() {
    let ctx = egui::Context::default();
    let mut app = animated_app(&ctx);
    for _ in 0..2 {
        frame(&ctx, vec![], |ui, ctx| {
            app.retire_fs_page_scroll_animation(ctx);
            paint(&mut app, ui, ctx);
        });
    }
    let expected = app.fs_page_scroll_animation.clone().unwrap();
    assert!(expected.start_time.is_some());
    assert!(expected.last_painted_time.is_some());
    assert_ne!(expected.last_painted_pan, expected.from);
    let sibling = app.build_window_context_for_test(1823002, |owner| {
        assert!(owner.fs_page_scroll_animation.is_none());
        owner.fs_pan = Vec2::new(23.0, 41.0);
    });
    let display_pan = app.fs_page_scroll_display_pan(&ctx);
    app.with_window_context_for_test(sibling, |owner| {
        // Service work may retire the mounted sibling's own effect. The active
        // reader's bundle stays at rest and keeps its original animation clock.
        owner.retire_fs_page_scroll_animation(&ctx);
        owner.fs_pan += Vec2::new(5.0, 7.0);
        assert!(owner.fs_page_scroll_animation.is_none());
    });
    assert_animation(&app, &expected);
    assert_eq!(app.fs_page_scroll_display_pan(&ctx), display_pan);
    app.with_window_context_for_test(sibling, |owner| {
        assert_eq!(owner.fs_pan, Vec2::new(28.0, 48.0));
        assert!(owner.fs_page_scroll_animation.is_none());
    });
    assert_animation(&app, &expected);
}

#[test]
fn landing_kind_and_items_generation_follow_only_their_context_owner() {
    let ctx = egui::Context::default();
    let mut app = animated_app(&ctx);
    let main_animation = app.fs_page_scroll_animation.clone().unwrap();
    app.request_fs_initial_placement_with_kind(0, FsPageLanding::End.initial_placement_kind());
    let main = app.fs_initial_placement_request.unwrap();
    let sibling = app.build_window_context_for_test(1823003, |owner| {
        owner.items_generation = main.items_generation + 1;
        owner.fullscreen_idx = Some(2);
        owner.request_fs_initial_placement_with_kind(
            2,
            FsPageLanding::Start.initial_placement_kind(),
        );
    });
    assert_eq!(app.fs_initial_placement_request, Some(main));
    assert_animation(&app, &main_animation);
    app.with_window_context_for_test(sibling, |owner| {
        let request = owner.fs_initial_placement_request.unwrap();
        assert_eq!(request.kind, FsPageLanding::Start.initial_placement_kind());
        assert_eq!(request.anchor_idx, 2);
        assert_eq!(request.items_generation, main.items_generation + 1);
        owner.clear_fs_initial_placement_request();
    });
    assert_eq!(app.fs_initial_placement_request, Some(main));
    assert_animation(&app, &main_animation);
    app.with_window_context_for_test(sibling, |owner| {
        assert!(owner.fs_initial_placement_request.is_none())
    });
    assert_eq!(app.fs_initial_placement_request, Some(main));
    assert_animation(&app, &main_animation);
}
