use super::handler_tests::{app, frame, seed, tap};
use super::*;

fn guarded_keys(app: &mut App, ctx: &egui::Context) -> FsKeyAction {
    let idx = app.fullscreen_idx.unwrap();
    canonical_demand::with_resident_metadata_only_for_test(|| {
        app.handle_fs_key_input(ctx, idx, false)
    })
}

fn assert_dropped_scroll(app: &mut App, ctx: &egui::Context) {
    let pan = app.fs_pan;
    frame(ctx, tap(egui::Key::K), |_, ctx| {
        assert!(guarded_keys(app, ctx).page_nav.is_none());
        assert_eq!(app.fs_pan, pan);
        assert!(!ctx.input(|input| input.events.iter().any(|event| matches!(
            event,
            egui::Event::Key {
                key: egui::Key::K,
                pressed: true,
                ..
            }
        ))));
    });
}

fn assert_resumed_scroll(app: &mut App, ctx: &egui::Context) {
    seed(ctx, app);
    let pan = app.fs_pan;
    frame(ctx, tap(egui::Key::K), |_, ctx| {
        assert!(guarded_keys(app, ctx).page_nav.is_none());
        assert_ne!(app.fs_pan, pan);
    });
}

#[test]
fn missing_rotation_is_consumed_without_metadata_production_and_new_paint_recovers() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, mode);
        app.fs_zoom = 3.0;
        seed(&ctx, &mut app);
        app.remove_rotation_cache_entry_for_reload(0);
        assert!(app.peek_rotation(0).is_none());
        assert_dropped_scroll(&mut app, &ctx);
        assert!(
            app.peek_rotation(0).is_none(),
            "input cannot fill the rotation cache"
        );
        assert_resumed_scroll(&mut app, &ctx);
    }
}

#[test]
fn missing_or_replaced_auto_trim_is_consumed_without_analysis_and_new_paint_recovers() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Single, SpreadMode::Ltr] {
        for replaced_pixels in [false, true] {
            let ctx = egui::Context::default();
            let mut app = app(&ctx, mode);
            app.fs_zoom = 3.0;
            app.settings.auto_view_trim_default_enabled = true;
            seed(&ctx, &mut app);
            assert!(app.fs_margin_bbox_cache.contains_key(&0));
            if replaced_pixels {
                let Some(FsCacheEntry::Static { pixels, .. }) = app.fs_cache.get_mut(&0) else {
                    panic!("static fixture");
                };
                *pixels = Arc::new((**pixels).clone());
            } else {
                app.fs_margin_bbox_cache.clear();
            }
            let cached = app.fs_margin_bbox_cache.clone();
            assert_dropped_scroll(&mut app, &ctx);
            assert_eq!(
                app.fs_margin_bbox_cache, cached,
                "input cannot analyze or replace a stale answer"
            );
            assert_resumed_scroll(&mut app, &ctx);
        }
    }
}

#[test]
fn missing_composition_is_not_rebuilt_by_input_and_new_paint_recovers() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Ltr);
    app.fs_zoom = 3.0;
    seed(&ctx, &mut app);
    app.spread_display_units_cache.token = None;
    assert_dropped_scroll(&mut app, &ctx);
    assert!(app.spread_display_units_cache.token.is_none());
    assert_resumed_scroll(&mut app, &ctx);
}

#[test]
fn ordinary_spread_does_not_require_an_unused_native_completeness_proof() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Ltr);
    app.current_folder = Some(app.tmp.path().to_path_buf());
    app.settings.final_cover_spread_enabled = false;
    app.settings.singleton_spread_first_enabled = false;
    app.settings.singleton_spread_last_enabled = false;
    app.fs_zoom = 3.0;
    seed(&ctx, &mut app);
    assert!(app.spread_complete_book_context_eligible());
    app.spread_display_units_cache.complete_page_permutation = None;
    let pan = app.fs_pan;
    frame(&ctx, tap(egui::Key::K), |_, ctx| {
        assert!(guarded_keys(&mut app, ctx).page_nav.is_none());
        assert_ne!(app.fs_pan, pan);
        assert!(
            app.spread_display_units_cache
                .complete_page_permutation
                .is_none()
        );
    });
}

#[test]
fn animated_auto_trim_retains_the_known_untrimmed_value_without_analysis() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.fs_zoom = 3.0;
    app.settings.auto_view_trim_default_enabled = true;
    let Some(FsCacheEntry::Static { tex, pixels, .. }) = app.fs_cache.get(&0) else {
        panic!("static source fixture");
    };
    let (tex, pixels) = (tex.clone(), pixels.clone());
    app.fs_cache.insert(
        0,
        FsCacheEntry::Animated {
            frames: vec![(tex.clone(), 1.0), (tex, 1.0)],
            frame_pixels: vec![pixels.clone(), pixels],
            current_frame: 0,
            playback: crate::fs_animation::AnimationPlayback::Paused,
            load_seq: 1,
        },
    );
    assert_resumed_scroll(&mut app, &ctx);
    assert!(!app.fs_margin_bbox_cache.contains_key(&0));
}

#[test]
fn missing_seek_geometry_is_not_populated_by_scroll_matcher_and_new_paint_recovers() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.fs_zoom = 3.0;
    app.settings.still_seek_strip_visible = true;
    seed(&ctx, &mut app);
    assert!(app.fullscreen_seek_overlay_allowed(0, false));
    assert!(app.viewer_navigation_caches.fs_seek_info.is_some());
    app.viewer_navigation_caches.fs_seek_info = None;
    let pan = app.fs_pan;
    // The existing key-handler prelude calls page_overview_available, which
    // prepares seek info. Exercise the scroll matcher before that producer.
    frame(&ctx, vec![], |_, ctx| {
        canonical_demand::with_resident_metadata_only_for_test(|| {
            assert!(!app.fs_scroll_layout_matches(ctx));
            app.apply_fs_page_scroll_key(ctx, KeyAction::FsPageNScrollNext);
        });
        assert_eq!(app.fs_pan, pan);
    });
    assert!(app.viewer_navigation_caches.fs_seek_info.is_none());
    assert_resumed_scroll(&mut app, &ctx);
}

#[test]
fn stale_generation_and_page_skip_geometry_without_loading_metadata() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for different_page in [false, true] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        seed(&ctx, &mut app);
        app.rotation_cache.clear();
        app.fs_margin_bbox_cache.clear();
        if different_page {
            app.fullscreen_idx = Some(1);
        } else {
            app.items_generation += 1;
        }
        assert_dropped_scroll(&mut app, &ctx);
        assert!(app.rotation_cache.is_empty());
        assert!(app.fs_margin_bbox_cache.is_empty());
    }
}

#[test]
fn rotation_popup_keeps_fixed_arrows_at_the_existing_replacement_point() {
    use crate::settings::FullscreenUpDownKeyAction;
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    for policy in [
        FullscreenUpDownKeyAction::PageTurn,
        FullscreenUpDownKeyAction::InPageScroll,
    ] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, SpreadMode::Single);
        app.settings.fullscreen_up_down_key_action = policy;
        seed(&ctx, &mut app);
        egui::Popup::open_id(&ctx, egui::Id::unique("fs_rotation_btn").with("popup"));
        frame(&ctx, tap(egui::Key::ArrowDown), |_, ctx| {
            assert!(fs_rotation_popup_open(ctx));
            let action = guarded_keys(&mut app, ctx);
            let replaced = policy == FullscreenUpDownKeyAction::InPageScroll;
            assert_eq!(action.page_nav.is_none(), replaced);
            assert_eq!(app.fs_pan.y < 0.0, replaced);
            assert!(!ctx.input(|input| input.events.iter().any(|event| matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    pressed: true,
                    ..
                }
            ))));
        });
    }
}
