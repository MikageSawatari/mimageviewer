//! Foreground handoff cancels detection before another context creates its draft.
#![cfg(windows)]

use super::tests::{mask_detect_install_source, mask_detect_output};
use super::*;
use crate::app::setup_app_for_test;

fn mask_detect_foreground_handoff_preserves_new_editor(close: bool) {
    for tool in [
        MaskDetectTool::Erase,
        MaskDetectTool::Conceal,
        MaskDetectTool::LocalAdjust,
    ] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        app.settings.detached_viewer_open_images_in_window = true;
        let main = app.projected_viewer_context_id();
        let old_path = app.tmp.path().join("old-detection-owner.png");
        let new_path = app.tmp.path().join("new-detection-owner.png");
        for path in [&old_path, &new_path] {
            image::RgbaImage::from_pixel(8, 8, image::Rgba([70, 100, 130, 255]))
                .save(path)
                .unwrap();
        }
        let owner = app.build_active_context_for_test(Some(914), DetachedSource::Image, |app| {
            mask_detect_install_source(app, old_path, tool, &ctx);
            app.viewer_presentation = ViewerPresentation::DetachedWindow;
            app.detached_viewer_independent_active = true;
            app.set_detached_window_binding_for_test(Some(914));
        });
        assert_ne!(owner, main);
        let old_cancel = Arc::new(AtomicBool::new(false));
        let (old_tx, old_rx) = mpsc::channel();
        app.with_viewer_context_state(owner, |app| {
            let request = app.capture_mask_detect_request(tool).unwrap();
            assert_eq!(request.context, owner);
            app.mask_detect_pending = Some(Box::new(MaskDetectPending {
                request,
                cancel: Arc::clone(&old_cancel),
                rx: old_rx,
            }));
        })
        .unwrap();
        assert_eq!(app.projected_viewer_context_id(), main);

        if close {
            app.close_and_retire_context(
                owner,
                "mask_detect_foreground_close_test",
                |app| {
                    app.close_fullscreen_content();
                    assert!(app.fullscreen_idx.is_none());
                    assert!(app.mask_detect_pending.is_none());
                },
                |_| (),
            )
            .unwrap();
            assert!(!app.viewer_context_ids().contains(&owner));
        } else {
            assert!(app.pause_current_active_viewer_context(&ctx));
            app.with_viewer_context_state(owner, |app| {
                assert!(app.mask_detect_pending.is_none());
                assert!(!app.local_adjust_mode);
            })
            .unwrap();
        }
        assert!(old_cancel.load(Ordering::Acquire));
        assert_eq!(app.projected_viewer_context_id(), main);
        assert!(!app.erase_mode && !app.conceal_mode);
        assert!(app.erase_shapes.is_empty() && app.conceal_shapes.is_empty());

        // Recreate the same tool through its normal entry boundary in the new owner.
        mask_detect_install_source(&mut app, new_path, tool, &ctx);
        app.erase_mode = false;
        app.conceal_mode = false;
        app.local_adjust_mode = false;
        match tool {
            MaskDetectTool::Erase => app.enter_erase_mode(0),
            MaskDetectTool::Conceal => {
                // The public start hydrates on a worker. Exercise its normal adoption
                // boundary with an empty loaded mask, without launching that worker.
                let (pixels, kind) = app.current_conceal_source_pixels(0).unwrap();
                assert!(app.complete_conceal_mode_entry(
                    0,
                    pixels,
                    kind,
                    vec![false; 64],
                    Vec::new(),
                ));
            }
            MaskDetectTool::LocalAdjust => app.enter_local_adjust_mode(&ctx),
        }
        let request = app.capture_mask_detect_request(tool).unwrap();
        assert_eq!(request.context, main);
        app.apply_mask_detect_output(&request, mask_detect_output());
        match tool {
            MaskDetectTool::Erase => assert_eq!(app.erase_shapes.len(), 1),
            MaskDetectTool::Conceal => assert_eq!(app.conceal_shapes.len(), 1),
            MaskDetectTool::LocalAdjust => {
                let local_adjust_core::LocalMask::RasterVector(mask) =
                    &app.local_adjust_page_layers[&0][0].mask
                else {
                    panic!("new editor must have a vector mask");
                };
                assert_eq!(mask.shapes.len(), 1);
            }
        }
        let new_request = app.capture_mask_detect_request(tool).unwrap();
        let source = Arc::clone(&new_request.source);
        let new_cancel = Arc::new(AtomicBool::new(false));
        let (_new_tx, new_rx) = mpsc::channel();
        app.mask_detect_pending = Some(Box::new(MaskDetectPending {
            request: new_request,
            cancel: Arc::clone(&new_cancel),
            rx: new_rx,
        }));
        let basis = crate::material_display_basis::MaterialDisplayBasis::from_sampler(
            [8, 8],
            crate::source_coverage::SourceCoverage::FullSource,
        );
        let local_key = app.current_local_adjust_key(0);
        match tool {
            MaskDetectTool::Erase => {
                app.erase_preview_cache.insert(
                    0,
                    ErasePreviewCacheEntry {
                        display_basis: basis,
                        pixels: Arc::clone(&source),
                        texture: ctx.load_texture(
                            "mask-detect-new-editor-preview",
                            source.as_ref().clone(),
                            Default::default(),
                        ),
                    },
                );
            }
            MaskDetectTool::Conceal => {
                let generation = app.conceal_generation;
                app.conceal_cache.insert(
                    0,
                    ConcealCacheEntry {
                        texture_options: Default::default(),
                        display_basis: basis,
                        pixels: Arc::clone(&source),
                        texture: None,
                        generation,
                    },
                );
            }
            MaskDetectTool::LocalAdjust => {
                app.local_adjust_cache.insert(
                    local_key,
                    LocalAdjustCacheEntry {
                        texture_options: Default::default(),
                        display_basis: basis,
                        pixels: Arc::clone(&source),
                        texture: None,
                    },
                );
            }
        }
        let erase_shapes = app.erase_shapes.clone();
        let conceal_shapes = app.conceal_shapes.clone();
        let erase_mask = app.erase_mask.clone();
        let conceal_mask = app.conceal_mask.clone();
        let layers = app.local_adjust_page_layers.clone();
        let history = (
            app.erase_undo_stack.len(),
            app.conceal_undo_stack.len(),
            app.meta_undo.undo_len(),
        );

        // A completed old inference cannot publish into the newly started editor.
        assert!(old_tx.send(Ok(mask_detect_output())).is_err());
        app.poll_all_page_edit_producers(&ctx);
        assert_eq!(app.projected_viewer_context_id(), main);
        assert_eq!(app.erase_shapes, erase_shapes);
        assert_eq!(app.conceal_shapes, conceal_shapes);
        assert_eq!(app.erase_mask, erase_mask);
        assert_eq!(app.conceal_mask, conceal_mask);
        assert_eq!(app.local_adjust_page_layers, layers);
        assert_eq!(
            (
                app.erase_undo_stack.len(),
                app.conceal_undo_stack.len(),
                app.meta_undo.undo_len(),
            ),
            history
        );
        let pending = app.mask_detect_pending.as_ref().unwrap();
        assert_eq!(pending.request.context, main);
        assert!(Arc::ptr_eq(&pending.cancel, &new_cancel));
        assert!(!new_cancel.load(Ordering::Acquire));
        let cached = match tool {
            MaskDetectTool::Erase => &app.erase_preview_cache[&0].pixels,
            MaskDetectTool::Conceal => &app.conceal_cache[&0].pixels,
            MaskDetectTool::LocalAdjust => &app.local_adjust_cache[&local_key].pixels,
        };
        assert!(Arc::ptr_eq(cached, &source));
    }
}

#[test]
fn mask_detect_actual_park_cancels_old_result_before_new_context_tool_entry() {
    mask_detect_foreground_handoff_preserves_new_editor(false);
}

#[test]
fn mask_detect_actual_close_cancels_old_result_before_new_context_tool_entry() {
    mask_detect_foreground_handoff_preserves_new_editor(true);
}
