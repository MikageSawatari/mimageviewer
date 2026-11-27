#![cfg(test)]

use super::*;
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::app::*;

#[derive(Clone, Copy, Debug)]
enum Tool {
    Erase,
    Conceal,
    Text,
    LocalAdjustment,
    Crop,
    SnsSplit,
}

const TOOLS: [Tool; 6] = [
    Tool::Erase,
    Tool::Conceal,
    Tool::Text,
    Tool::LocalAdjustment,
    Tool::Crop,
    Tool::SnsSplit,
];

fn page(ctx: &egui::Context) -> AppTestEnv {
    let mut app = setup_app();
    let path = app.tmp.path().join("edit-admission.png");
    image::RgbaImage::from_pixel(4, 6, image::Rgba([90, 100, 110, 255]))
        .save(&path)
        .unwrap();
    app.items = vec![GridItem::Image(path)];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.image_metas = vec![None];
    app.visible_indices = vec![0];
    app.fullscreen_idx = Some(0);
    app.spread_mode = crate::settings::SpreadMode::Single;
    app.viewer_navigation_caches.invalidate();
    let pixels = Arc::new(egui::ColorImage::filled([4, 6], egui::Color32::WHITE));
    let tex = ctx.load_texture("page-edit-admission", pixels.clone(), Default::default());
    app.fs_cache.insert(
        0,
        FsCacheEntry::Static {
            pdf_rerender_failures: Vec::new(),
            pdf_metadata: None,
            tex,
            pixels,
            source_dims: Some([4, 6]),
            load_seq: 0,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    app.rotation_cache
        .insert(0, crate::rotation_db::Rotation::None);
    app.comic_font_registry_loaded = true;
    app.comic_fonts.seed_for_test(
        Some(Arc::new(comic_core::FontSet::new())),
        Default::default(),
    );
    app.editing_addon_declined_session = true;
    app.show_whats_new = false;
    app.show_mouse_nav_migration_prompt = false;
    app.network_data_dir_notice = None;
    app
}

fn enter(app: &mut App, ctx: &egui::Context, tool: Tool) {
    match tool {
        Tool::Erase => app.enter_erase_mode(0),
        Tool::Conceal => app.enter_conceal_mode(ctx, 0),
        Tool::Text => app.enter_text_mode(0),
        Tool::LocalAdjustment => app.enter_local_adjust_mode(ctx),
        Tool::Crop => app.enter_export_crop_mode(0),
        Tool::SnsSplit => {
            let _ = app.enter_sns_split_mode(0);
        }
    }
}

fn active(app: &App, tool: Tool) -> bool {
    match tool {
        Tool::Erase => app.erase_mode,
        Tool::Conceal => app.conceal_mode,
        Tool::Text => app.text_mode,
        Tool::LocalAdjustment => app.local_adjust_mode,
        Tool::Crop => app.export_crop_mode,
        Tool::SnsSplit => app.sns_split.is_some(),
    }
}

#[test]
fn page_edit_admission_all_six_tools_stop_slideshow_and_block_restart() {
    for tool in TOOLS {
        let ctx = egui::Context::default();
        let mut app = page(&ctx);
        app.slideshow_playing = true;
        app.slideshow_anchor_idx = Some(0);
        enter(&mut app, &ctx, tool);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !active(&app, tool) && !app.local_adjust_pending.is_empty() {
            app.poll_local_adjust_render(&ctx);
            assert!(Instant::now() < deadline, "{tool:?} entry timed out");
            std::thread::yield_now();
        }
        assert!(active(&app, tool), "{tool:?}");
        assert!(!app.slideshow_playing, "{tool:?}");
        assert_eq!(app.slideshow_anchor_idx, None, "{tool:?}");
        app.toggle_fullscreen_slideshow(0);
        assert!(!app.slideshow_playing, "{tool:?} cannot restart slideshow");
    }
}

#[test]
fn page_edit_admission_all_six_tools_wait_for_lock_and_folder_navigation() {
    for navigation in ["lock", "manual_folder", "slideshow_folder"] {
        for tool in TOOLS {
            let ctx = egui::Context::default();
            let mut app = page(&ctx);
            let (_tx, rx) = mpsc::channel();
            if navigation != "lock" {
                app.folder_nav_pending = Some(FolderNavPending {
                    cancel: Arc::new(AtomicBool::new(false)),
                    rx,
                    forward: true,
                    mode: if navigation == "manual_folder" {
                        FolderNavMode::Fullscreen
                    } else {
                        FolderNavMode::SlideshowNext
                    },
                });
            } else {
                app.fs_nav_locked_gen = Some(app.items_generation);
            }
            enter(&mut app, &ctx, tool);
            assert!(
                !app.is_overlay_edit_mode_active(),
                "{tool:?} navigation={navigation}"
            );
            assert!(app.local_adjust_pending.is_empty());
            assert_eq!(app.fullscreen_idx, Some(0));
            assert_eq!(
                app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
                Some(PAGE_EDIT_NAVIGATION_PENDING_NOTICE)
            );
            app.folder_nav_pending = None;
            app.fs_nav_locked_gen = None;
            assert!(app.fullscreen_edit_mode_entry_allowed(0));
        }
    }
}

#[test]
fn page_edit_admission_deferred_folder_reopen_waits_even_after_worker_is_drained() {
    let ctx = egui::Context::default();
    let mut app = page(&ctx);
    app.fs_nav_after_pdf_enumerate = Some(DeferredFsReopen {
        viewer_adoption: ViewerOpenAdoption::ContextContinuation,
        history_trigger: HistoryTrigger::UserChosen,
        resume_slideshow: true,
        target: DeferredFsTarget::None,
        resume_to_last_page: false.into(),
        from_explicit_open: false,
        preserve_after_password_prompt: false,
    });
    assert!(app.folder_nav_pending.is_none());
    assert!(!app.fs_nav_is_locked());
    assert!(!app.fullscreen_edit_mode_entry_allowed(0));
    app.fs_nav_after_pdf_enumerate = None;
    assert!(app.fullscreen_edit_mode_entry_allowed(0));
}

#[test]
fn page_edit_admission_collection_continuation_waits_without_fs_lock() {
    let ctx = egui::Context::default();
    let mut app = page(&ctx);
    app.top_level_grid_view
        .set_collection_navigation_pending(Some(
            collection_navigation::CollectionNavigationPending::AwaitingOuterContinuation {
                steps: 1,
                fullscreen: true,
                resume_slideshow: true,
                native_toast: false,
            },
        ));
    assert!(!app.fs_nav_is_locked());
    assert!(!app.fullscreen_edit_mode_entry_allowed(0));
    app.top_level_grid_view.take_collection_navigation_pending();
    assert!(app.fullscreen_edit_mode_entry_allowed(0));
}

#[test]
fn page_edit_admission_async_original_completion_rechecks_navigation() {
    for entry in [
        OriginalMaterialEntry::Erase {
            requested_fs_idx: 0,
        },
        OriginalMaterialEntry::Conceal {
            requested_fs_idx: 0,
        },
        OriginalMaterialEntry::LocalAdjustment {
            requested_fs_idx: 0,
        },
    ] {
        let ctx = egui::Context::default();
        let mut app = page(&ctx);
        app.fs_nav_locked_gen = Some(app.items_generation);
        app.resume_original_material_entry(&ctx, 0, entry);
        assert!(!app.is_overlay_edit_mode_active());
        assert!(app.local_adjust_pending.is_empty());
        assert_eq!(
            app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
            Some(PAGE_EDIT_NAVIGATION_PENDING_NOTICE)
        );
    }
}

#[test]
fn page_edit_admission_conceal_material_completion_rechecks_navigation_then_can_enter() {
    let ctx = egui::Context::default();
    let mut app = page(&ctx);
    let pixels = Arc::new(egui::ColorImage::filled([4, 6], egui::Color32::WHITE));
    app.fs_nav_locked_gen = Some(app.items_generation);
    assert!(!app.complete_conceal_mode_entry(0, pixels.clone(), "test", vec![false; 24], vec![]));
    assert!(!app.conceal_mode);
    assert!(app.conceal_base_cache.is_empty());
    app.fs_nav_locked_gen = None;
    app.slideshow_playing = true;
    assert!(app.complete_conceal_mode_entry(0, pixels, "test", vec![false; 24], vec![]));
    assert!(app.conceal_mode);
    assert!(!app.slideshow_playing);
}

#[cfg(windows)]
#[test]
fn page_edit_admission_sibling_projection_cannot_acquire_foreground_draft() {
    let ctx = egui::Context::default();
    let mut app = page(&ctx);
    let main = app.viewer_context_main();
    let sibling = app.build_window_context_for_test(192035, |_| {});
    app.conceal_mode = true;
    app.conceal_mask_size = [4, 6];
    app.conceal_mask = Some(vec![true; 24]);
    let path = app.tmp.path().join("sibling.png");
    app.with_viewer_context(sibling, |projected| {
        projected.items = vec![GridItem::Image(path)];
        projected.fullscreen_idx = Some(0);
        assert!(!projected.page_edit_mode_navigation_entry_allowed());
        let pixels = Arc::new(egui::ColorImage::filled([2, 3], egui::Color32::BLACK));
        assert!(!projected.complete_conceal_mode_entry(
            0,
            pixels,
            "sibling-test",
            vec![false; 6],
            vec![],
        ));
        // The foreground draft must not prohibit this independent slideshow.
        projected.toggle_fullscreen_slideshow(0);
        assert!(projected.slideshow_playing);
        assert!(projected.fs_feedback_toast.is_none());
    })
    .unwrap();
    assert_eq!(app.projected_viewer_context_id(), main);
    assert!(app.conceal_mode);
    assert_eq!(app.conceal_mask_size, [4, 6]);
    assert_eq!(app.conceal_mask, Some(vec![true; 24]));
    assert!(!app.slideshow_playing);
    assert!(app.page_edit_mode_navigation_entry_allowed());
}
