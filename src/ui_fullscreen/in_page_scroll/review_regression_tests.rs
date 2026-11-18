use super::handler_tests::{app, frame, full_rect, paint, run_in_viewport, seed, tap};
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
fn root_dispatched_scroll_keys_leave_the_destination_rotation_popup_unconsumed() {
    #[cfg(windows)]
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.native_video_in_window_active = false;
    app.settings.video_in_window_mode = false;
    app.viewer_presentation = crate::app::ViewerPresentation::Fullscreen;
    let destination = app.fullscreen_viewport_id();
    assert_ne!(destination, egui::ViewportId::ROOT);
    ctx.set_embed_viewports(false);
    let keep_destination = |ctx: &egui::Context| {
        ctx.show_viewport_deferred(destination, Default::default(), |_, _| {});
    };
    frame(&ctx, vec![], |_, ctx| keep_destination(ctx));
    let popup = egui::Id::unique("fs_rotation_btn").with("popup");
    run_in_viewport(&ctx, destination, full_rect(), 1.0, vec![], |ui, ctx| {
        paint(&mut app, ui, ctx);
        egui::Popup::open_id(ctx, popup);
        assert!(ctx.memory(|memory| memory.focused().is_none()));
    });
    let pan = app.fs_pan;
    for key in [egui::Key::K, egui::Key::J] {
        frame(&ctx, tap(key), |_, ctx| {
            keep_destination(ctx);
            assert_eq!(
                egui::Popup::current_open_id_for(ctx, destination),
                Some(popup)
            );
            assert!(!egui::Popup::is_any_open(ctx));
            assert!(
                !app.keyboard_owner_for_pass(ctx)
                    .blocks_legacy_keymap_shortcuts()
            );
            assert!(app.fs_scroll_layout_matches(ctx));
            assert!(app.handle_fullscreen_root_key_input(ctx));
            assert_eq!(app.fs_pan, pan);
            assert!(ctx.input(|input| input.events.iter().any(|event| matches!(
                event, egui::Event::Key { key: actual, pressed: true, .. } if *actual == key
            ))));
        });
    }
    run_in_viewport(&ctx, destination, full_rect(), 1.0, vec![], |_, ctx| {
        egui::Popup::close_id(ctx, popup);
    });
    frame(&ctx, tap(egui::Key::K), |_, ctx| {
        keep_destination(ctx);
        assert!(app.handle_fullscreen_root_key_input(ctx));
        assert_ne!(
            app.fs_pan, pan,
            "the same ROOT route works after closing the popup"
        );
    });
    let pan = app.fs_pan;
    frame(&ctx, tap(egui::Key::K), |_, ctx| {
        keep_destination(ctx);
        egui::Popup::open_id(ctx, egui::Id::unique("scroll-root-front-ui"));
        assert_eq!(egui::Popup::current_open_id_for(ctx, destination), None);
        assert!(app.handle_fullscreen_root_key_input(ctx));
        assert_eq!(app.fs_pan, pan, "the ingress front-UI gate is preserved");
        assert!(ctx.input(|input| input.events.iter().any(|event| matches!(
            event,
            egui::Event::Key {
                key: egui::Key::K,
                pressed: true,
                ..
            }
        ))));
    });
}

#[cfg(windows)]
#[test]
fn detached_raw_auto_trim_uses_the_painted_pixel_cache_and_keeps_runtime_priority() {
    let _serial = crate::key_input::lock_test_input();
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.auto_view_trim_default_enabled = true;
    app.native_video_in_window_active = false;
    app.settings.video_in_window_mode = false;
    let mut image = egui::ColorImage::filled([80, 120], egui::Color32::WHITE);
    for y in 12..108 {
        for x in 8..72 {
            image.pixels[y * 80 + x] = egui::Color32::BLACK;
        }
    }
    let pixels = Arc::new(image);
    let tex = ctx.load_texture(
        "detached-raw-scroll",
        pixels.clone(),
        egui::TextureOptions::LINEAR,
    );
    let window = 47402;
    let sibling = app.build_window_context_for_test(window, |owner| {
        owner.items = vec![GridItem::Image(PathBuf::from("c:/scroll/preview.arw"))];
        owner.visible_indices = vec![0];
        owner.thumbnails = vec![ThumbnailState::Pending];
        owner.fullscreen_idx = Some(0);
        owner.viewer_presentation = crate::app::ViewerPresentation::DetachedWindow;
        owner.reading_flow = ReadingFlow::Paged;
        owner.spread_mode = SpreadMode::Single;
        owner.fs_zoom = 3.0;
        owner
            .rotation_cache
            .insert(0, crate::rotation_db::Rotation::None);
        owner.fs_cache.insert(
            0,
            FsCacheEntry::RawPreview {
                preview: Some(crate::fs_animation::RawPreviewTexture { tex, pixels }),
                developed_dims: [800, 1200],
                load_seq: 123,
            },
        );
        owner.record_page_dims_for_spread(0, (800, 1200));
    });
    app.with_window_context_for_test(sibling, |owner| {
        assert!(owner.viewer_session_is_detached());
        assert_eq!(owner.detached_viewer_window_id(), Some(window));
        assert!(owner.is_raw_page(0));
        let destination = owner.fullscreen_viewport_id();
        run_in_viewport(
            &ctx,
            destination,
            full_rect(),
            1.0,
            tap(egui::Key::K),
            |_, ctx| {
                let pan = owner.fs_pan;
                assert!(guarded_keys(owner, ctx).page_nav.is_none());
                assert_eq!(owner.fs_pan, pan);
                assert!(!ctx.input(|input| input.key_pressed(egui::Key::K)));
                assert!(owner.fs_margin_bbox_cache.is_empty());
            },
        );
        run_in_viewport(&ctx, destination, full_rect(), 1.0, vec![], |ui, ctx| {
            paint(owner, ui, ctx);
        });
        let painted = owner.fs_margin_bbox_cache[&0].2;
        assert!(painted.is_some());
        assert_eq!(owner.detached_window_runtime_trim_bbox(window, 0), None);
        run_in_viewport(
            &ctx,
            destination,
            full_rect(),
            1.0,
            tap(egui::Key::K),
            |_, ctx| {
                let pan = owner.fs_pan;
                assert!(guarded_keys(owner, ctx).page_nav.is_none());
                assert_ne!(owner.fs_pan, pan);
            },
        );
        // Both load identity and pixel identity must still match the paint.
        let Some(FsCacheEntry::RawPreview { load_seq, .. }) = owner.fs_cache.get_mut(&0) else {
            unreachable!()
        };
        *load_seq += 1;
        assert_eq!(owner.fs_scroll_resident_auto_trim(0), None);
        let Some(FsCacheEntry::RawPreview {
            load_seq,
            preview: Some(preview),
            ..
        }) = owner.fs_cache.get_mut(&0)
        else {
            unreachable!()
        };
        *load_seq = 123;
        preview.pixels = Arc::new((*preview.pixels).clone());
        assert_eq!(owner.fs_scroll_resident_auto_trim(0), None);
        owner.set_detached_window_runtime_trim_bbox(window, 0, None, "scroll-test");
        assert_eq!(owner.fs_scroll_resident_auto_trim(0), Some(None));
        assert_eq!(owner.fs_margin_bbox_cache[&0].2, painted);
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
fn cached_and_ordinary_composition_share_completeness_and_page_alone_policy() {
    use crate::settings::PageAloneSettings;
    // Cover, endpoint placement and page-alone requests independently need a
    // completeness proof. Page-alone requests outside cover mode are inert.
    for (mode, cover, first, last, after_cover, last_alone, needs_proof) in [
        (SpreadMode::Ltr, false, false, false, false, false, false),
        (SpreadMode::Ltr, true, false, false, true, true, false),
        (SpreadMode::Ltr, false, true, false, false, false, true),
        (SpreadMode::Rtl, false, false, true, false, false, true),
        (SpreadMode::LtrCover, true, false, false, false, false, true),
        (SpreadMode::RtlCover, false, false, false, true, false, true),
        (SpreadMode::LtrCover, false, false, false, false, true, true),
        (
            SpreadMode::RtlCover,
            false,
            false,
            false,
            false,
            false,
            false,
        ),
    ] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, mode);
        app.current_folder = Some(app.tmp.path().to_path_buf());
        app.settings.final_cover_spread_enabled = cover;
        app.settings.singleton_spread_first_enabled = first;
        app.settings.singleton_spread_last_enabled = last;
        app.settings.page_after_cover_alone_enabled = after_cover;
        app.settings.last_page_alone_enabled = last_alone;
        let ordinary = app.spread_display_composition_for_anchor(0);
        assert_eq!(
            app.peek_spread_display_composition_for_anchor(0),
            Some(ordinary)
        );
        let effective = app
            .spread_display_units_cache
            .token
            .as_ref()
            .unwrap()
            .page_alone;
        assert_eq!(
            effective,
            if mode.has_cover() {
                PageAloneSettings {
                    after_cover,
                    last: last_alone,
                }
            } else {
                PageAloneSettings::default()
            }
        );
        app.spread_display_units_cache.complete_page_permutation = None;
        assert_eq!(
            app.peek_spread_display_composition_for_anchor(0).is_none(),
            needs_proof
        );
        // Only the ordinary producer may acquire the missing proof.
        let ordinary = app.spread_display_composition_for_anchor(0);
        assert_eq!(
            app.peek_spread_display_composition_for_anchor(0),
            Some(ordinary)
        );
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
