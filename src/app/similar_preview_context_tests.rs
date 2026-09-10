#![cfg(all(test, windows))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::fs_animation::{FsCacheEntry, StaticAnimationState};
use crate::grid_item::{GridItem, ThumbnailState};

fn preview_hit(path: &Path, mtime: i64) -> crate::similar_index::QueryHit {
    crate::similar_index::QueryHit {
        item_id: mtime as u64,
        item_key: crate::similar_index::item_key_for_file(path),
        kind: crate::similar_db::ItemKind::Image,
        container_key: None,
        page_index: None,
        distance: 1,
        band: crate::similar_index::MatchBand::NearlyIdentical,
        mtime,
        file_size: 20,
        width: 32,
        height: 24,
        format: crate::similar_image::SimilarImageFormat::Png,
        target: Some(crate::similar_index::SimilarItemTarget::File(
            path.to_path_buf(),
        )),
    }
}

fn preview_ready_query(hit: crate::similar_index::QueryHit) -> crate::similar_index::ItemQuery {
    crate::similar_index::ItemQuery::Ready(crate::similar_index::ItemMatches {
        origin: crate::similar_index::OriginItem {
            item_key: "preview-origin".to_owned(),
            kind: crate::similar_db::ItemKind::Image,
            mtime: 0,
            file_size: 1,
            width: 1,
            height: 1,
            format: crate::similar_image::SimilarImageFormat::Png,
            target: None,
        },
        hits: vec![hit],
    })
}

fn seed_still_viewer(app: &mut App, ctx: &egui::Context, path: &str) -> usize {
    let path = PathBuf::from(path);
    let idx = app.items.len();
    app.items.push(GridItem::Image(path.clone()));
    app.thumbnails.push(ThumbnailState::Pending);
    app.image_metas.push(None);
    app.visible_indices.push(idx);
    app.fullscreen_idx = Some(idx);
    app.current_folder = path.parent().map(Path::to_path_buf);
    app.address = path.to_string_lossy().into_owned();
    let pixels = Arc::new(egui::ColorImage::filled([4, 3], egui::Color32::GRAY));
    let texture = ctx.load_texture(
        format!("similar-preview-owner-{idx}"),
        Arc::clone(&pixels),
        egui::TextureOptions::LINEAR,
    );
    app.fs_cache.insert(
        idx,
        FsCacheEntry::Static {
            tex: texture,
            pixels,
            source_dims: Some([400, 300]),
            load_seq: 1,
            animation: StaticAnimationState::Still,
        },
    );
    idx
}

fn begin_preview(
    app: &mut App,
    ctx: &egui::Context,
    hit: &crate::similar_index::QueryHit,
) -> crate::similar_preview::SimilarPreviewTestCompletion {
    let session = app
        .similar_preview_session(ctx)
        .expect("still viewer preview session");
    assert!(app.similar_panel.preview.begin_test_press(
        ctx,
        hit,
        &app.pdf_passwords,
        crate::pdf_loader::PdfDisplayTarget {
            width_px: 800,
            height_px: 600,
            fit_mode: crate::pdf_loader::PdfDisplayFitMode::Page,
        },
        session,
    ));
    app.similar_panel.preview.take_test_completion()
}

fn complete_late(
    completion: crate::similar_preview::SimilarPreviewTestCompletion,
) -> Result<(), String> {
    completion.send_prepared(
        Arc::new(egui::ColorImage::filled([3, 2], egui::Color32::LIGHT_BLUE)),
        [300, 200],
        None,
        1,
    )
}

fn assert_preview_drained(app: &App) {
    assert!(!app.similar_panel.preview.has_pending_worker_for_test());
    assert!(!app.similar_panel.preview.has_active_gesture());
    assert_eq!(app.similar_panel.preview.cached_paint_resource_id(), None);
}

#[test]
fn similar_preview_source_notification_rejects_late_completion_in_mounted_context() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let path = PathBuf::from(r"C:\preview\source.png");
    seed_still_viewer(&mut app, &ctx, path.to_str().unwrap());
    let old_hit = preview_hit(&path, 1);
    let new_hit = preview_hit(&path, 2);
    let completion = begin_preview(&mut app, &ctx, &old_hit);

    let passwords = app.pdf_passwords.clone();
    app.similar_panel.preview.observe_query_result(
        &preview_ready_query(new_hit),
        &passwords,
        viewport_target(),
    );
    assert!(app.similar_panel.preview.has_pending_worker_for_test());
    assert!(!app.similar_panel.preview.has_active_gesture());
    complete_late(completion).expect("draining mounted receiver remains owned");

    app.poll_similar_preview_workers_in_all_contexts(&ctx);
    assert_preview_drained(&app);
}

#[test]
fn similar_preview_actual_file_change_rejects_and_drains_late_completion() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let path = PathBuf::from(r"C:\preview\file-change.png");
    seed_still_viewer(&mut app, &ctx, path.to_str().unwrap());
    let completion = begin_preview(&mut app, &ctx, &preview_hit(&path, 1));

    app.reset_fs_side_panel_runtime_for_file_change();
    assert!(app.similar_panel.preview.has_pending_worker_for_test());
    assert!(!app.similar_panel.preview.has_active_gesture());
    complete_late(completion).expect("draining file-change receiver remains owned");

    app.poll_similar_preview_workers_in_all_contexts(&ctx);
    assert_preview_drained(&app);
}

#[test]
fn similar_preview_true_close_rejects_and_drains_late_completion_in_mounted_context() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let path = PathBuf::from(r"C:\preview\close.png");
    seed_still_viewer(&mut app, &ctx, path.to_str().unwrap());
    let completion = begin_preview(&mut app, &ctx, &preview_hit(&path, 1));

    app.close_fullscreen();
    assert_eq!(app.fullscreen_idx, None);
    assert!(app.similar_panel.preview.has_pending_worker_for_test());
    complete_late(completion).expect("draining close receiver remains owned");

    app.poll_similar_preview_workers_in_all_contexts(&ctx);
    assert_preview_drained(&app);
}

#[test]
fn similar_preview_actual_park_drains_late_completion_while_context_stays_at_rest() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    app.settings.detached_viewer_open_images_in_window = true;
    let root_path = PathBuf::from(r"C:\preview\root-stays-current.png");
    seed_still_viewer(&mut app, &ctx, root_path.to_str().unwrap());
    let root_completion = begin_preview(&mut app, &ctx, &preview_hit(&root_path, 11));

    let window_id = 410;
    let path = PathBuf::from(r"C:\preview\park.png");
    let mut completion = None;
    let context_id = app.build_active_context_for_test(
        Some(window_id),
        crate::app::DetachedSource::Image,
        |viewer| {
            let idx = seed_still_viewer(viewer, &ctx, path.to_str().unwrap());
            viewer.fullscreen_idx = Some(idx);
            viewer.viewer_presentation = ViewerPresentation::DetachedWindow;
            viewer.detached_viewer_independent_active = true;
            viewer.fs_viewport_shown = true;
            viewer.fs_viewport_presentation = Some(ViewerPresentation::DetachedWindow);
            completion = Some(begin_preview(viewer, &ctx, &preview_hit(&path, 1)));
        },
    );
    assert_eq!(
        app.viewer_context_residence(context_id),
        ContextResidence::AtRest
    );

    assert!(app.pause_current_active_viewer_context(&ctx));
    assert_eq!(
        app.viewer_context_residence(context_id),
        ContextResidence::AtRest
    );
    complete_late(completion.expect("park completion handle"))
        .expect("parked draining receiver remains owned");
    complete_late(root_completion).expect("unrelated mounted receiver remains owned");

    app.poll_similar_preview_workers_in_all_contexts(&ctx);
    assert_eq!(
        app.viewer_context_residence(context_id),
        ContextResidence::AtRest,
        "background preview polling must not mount the parked viewer"
    );
    assert!(
        app.similar_panel
            .preview
            .cached_paint_resource_id()
            .is_some(),
        "parking A must not reject B's current completion"
    );
    assert!(app.similar_panel.preview.has_active_gesture());
    assert!(!app.similar_panel.preview.has_pending_worker_for_test());
    app.with_viewer_context(context_id, |parked| {
        assert_preview_drained(parked);
    })
    .expect("inspect parked viewer after background poll");
}

fn viewport_target() -> crate::pdf_loader::PdfDisplayTarget {
    crate::pdf_loader::PdfDisplayTarget {
        width_px: 800,
        height_px: 600,
        fit_mode: crate::pdf_loader::PdfDisplayFitMode::Page,
    }
}
