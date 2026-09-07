use super::*;
use crate::rotation_db::Rotation;

fn page_texture(ctx: &egui::Context, idx: usize, rotation: Rotation) -> egui::TextureHandle {
    // All displayed pages are portrait, but the raw landscape pixels for 90/270
    // make both the orientation and bubble-size regressions observable.
    let size = match rotation {
        Rotation::Cw90 | Rotation::Cw270 => [120, 80],
        _ => [80, 120],
    };
    let mut image = egui::ColorImage::filled(size, egui::Color32::WHITE);
    for y in 0..size[1] {
        for x in 0..size[0] {
            image[(x, y)] = match (x < size[0] / 2, y < size[1] / 2) {
                (true, true) => egui::Color32::from_rgb(240, 70, 50),
                (false, true) => egui::Color32::from_rgb(60, 190, 80),
                (false, false) => egui::Color32::from_rgb(60, 110, 235),
                (true, false) => egui::Color32::from_rgb(235, 200, 40),
            };
        }
    }
    ctx.load_texture(
        format!("rotation-page-{idx}"),
        image,
        egui::TextureOptions::NEAREST,
    )
}

fn load_textures(app: &mut crate::app::App, ctx: &egui::Context, rotations: &[Rotation]) {
    app.thumbnails = rotations
        .iter()
        .enumerate()
        .map(|(idx, &rotation)| {
            let tex = page_texture(ctx, idx, rotation);
            ThumbnailState::Loaded {
                source_dims: Some((tex.size()[0] as u32, tex.size()[1] as u32)),
                layout_dims: None,
                rendered_at_px: 120,
                from_cache: false,
                from_edit_preview: false,
                tex,
            }
        })
        .collect();
}

fn texture_meshes(output: &egui::FullOutput, id: egui::TextureId) -> Vec<egui::epaint::Mesh> {
    fn visit(shape: &egui::Shape, id: egui::TextureId, meshes: &mut Vec<egui::epaint::Mesh>) {
        match shape {
            egui::Shape::Mesh(mesh) if mesh.texture_id == id => meshes.push((**mesh).clone()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| visit(s, id, meshes)),
            _ => {}
        }
    }
    let mut meshes = Vec::new();
    for shape in &output.shapes {
        visit(&shape.shape, id, &mut meshes);
    }
    meshes
}

fn overlay_output(
    app: &mut crate::app::App,
    ctx: &egui::Context,
    hover: Option<egui::Pos2>,
) -> egui::FullOutput {
    let before = crate::rotation_db::read_operations_for_test();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            events: hover.into_iter().map(egui::Event::PointerMoved).collect(),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.draw_fullscreen_seek_overlay(ui, ctx, ctx.content_rect(), 3, false, false);
            });
        },
    );
    assert_eq!(
        crate::rotation_db::read_operations_for_test(),
        before,
        "the production overlay must not open/read the rotation DB on its calling thread"
    );
    output
}

fn preview_output(app: &mut crate::app::App, ctx: &egui::Context) -> egui::FullOutput {
    overlay_output(app, ctx, None);
    let geometry = app.still_seek_geometry_for_idx(3, false);
    let full = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    let strip = geometry.strip_rect(full).unwrap();
    let lock = crate::video::seek_strip_layout::seek_strip_lock_button_rect(strip);
    let center = egui::pos2(
        (strip.left() + 6.0 + lock.left() - 6.0) * 0.5,
        strip.center().y,
    );
    overlay_output(app, ctx, Some(center));
    overlay_output(app, ctx, Some(center))
}

fn assert_mesh_rotation(mesh: &egui::epaint::Mesh, rotation: Rotation) {
    let bounds = mesh.calc_bounds();
    let vertex = mesh
        .vertices
        .iter()
        .find(|v| v.pos == bounds.left_top())
        .unwrap();
    let expected = match rotation {
        Rotation::None => egui::pos2(0.0, 0.0),
        Rotation::Cw90 => egui::pos2(0.0, 1.0),
        Rotation::Cw180 => egui::pos2(1.0, 1.0),
        Rotation::Cw270 => egui::pos2(1.0, 0.0),
    };
    assert_eq!(
        vertex.uv, expected,
        "saved {rotation:?} must reach the real preview mesh"
    );
}

#[test]
fn still_seek_f5_drawing_does_not_synchronously_fill_rotation_cache() {
    let mut app = still_seek_edge_test_app();
    let ctx = egui::Context::default();
    load_textures(&mut app, &ctx, &[Rotation::Cw90; 10]);
    app.rotation_cache.clear();
    let output = overlay_output(&mut app, &ctx, None);
    assert!(
        app.rotation_cache.is_empty(),
        "drawing must enqueue a worker, not read SQLite inline"
    );
    let ThumbnailState::Loaded { tex, .. } = &app.thumbnails[3] else {
        unreachable!()
    };
    assert!(
        texture_meshes(&output, tex.id()).is_empty(),
        "unknown rotation must stop the strip before placing the cell"
    );
}

#[test]
fn still_seek_f5_strip_has_no_synchronous_rotation_lookup() {
    let source = include_str!("../../ui_fullscreen.rs");
    let start = source
        .find("let rotation_candidates = still_seek_strip_rotation_candidates(")
        .unwrap();
    let end = start + source[start..].find("let row_rect = layout").unwrap();
    assert!(
        !source[start..end].contains("get_rotations_for_indices"),
        "strip drawing must only read the memory cache and request a worker"
    );
}

#[test]
fn still_seek_f6_real_preview_mesh_matches_saved_rotation() {
    for rotation in [
        Rotation::Cw90,
        Rotation::Cw180,
        Rotation::Cw270,
        Rotation::None,
    ] {
        let mut app = still_seek_edge_test_app();
        let ctx = egui::Context::default();
        load_textures(&mut app, &ctx, &[rotation; 10]);
        for idx in 0..10 {
            app.rotation_cache.insert(idx, rotation);
        }
        app.settings.still_seek_hover_preview_mode =
            crate::settings::StillSeekHoverPreviewMode::Always;
        let output = preview_output(&mut app, &ctx);
        let ThumbnailState::Loaded { tex, .. } = &app.thumbnails[3] else {
            unreachable!()
        };
        let meshes = texture_meshes(&output, tex.id());
        assert_eq!(
            meshes.len(),
            2,
            "both production strip and preview must contain the real texture"
        );
        for mesh in meshes {
            assert_mesh_rotation(&mesh, rotation);
        }
    }
}

#[test]
fn still_seek_f6_real_preview_dimensions_follow_rotated_aspect() {
    for rotation in [
        Rotation::Cw90,
        Rotation::Cw270,
        Rotation::Cw180,
        Rotation::None,
    ] {
        let mut app = still_seek_edge_test_app();
        let ctx = egui::Context::default();
        load_textures(&mut app, &ctx, &[rotation; 10]);
        for idx in 0..10 {
            app.rotation_cache.insert(idx, rotation);
        }
        app.settings.still_seek_hover_preview_mode =
            crate::settings::StillSeekHoverPreviewMode::Always;
        let output = preview_output(&mut app, &ctx);
        let ThumbnailState::Loaded { tex, .. } = &app.thumbnails[3] else {
            unreachable!()
        };
        let meshes = texture_meshes(&output, tex.id());
        assert_eq!(meshes.len(), 2);
        let bounds = meshes.last().unwrap().calc_bounds();
        assert!(
            (bounds.aspect_ratio() - 2.0 / 3.0).abs() < 0.001,
            "{rotation:?}: {bounds:?}"
        );
        assert!((bounds.height() - STILL_SEEK_PREVIEW_MAX_HEIGHT).abs() < 0.001);
    }
}

#[test]
fn still_seek_f6_spread_preview_rotates_each_real_texture() {
    for spread in [
        crate::settings::SpreadMode::Ltr,
        crate::settings::SpreadMode::Rtl,
    ] {
        let mut app = still_seek_edge_test_app();
        let ctx = egui::Context::default();
        let rotations = [
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
            Rotation::Cw90,
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw90,
            Rotation::Cw90,
            Rotation::Cw90,
            Rotation::Cw90,
        ];
        load_textures(&mut app, &ctx, &rotations);
        for (idx, &rotation) in rotations.iter().enumerate() {
            app.rotation_cache.insert(idx, rotation);
        }
        app.spread_mode = spread;
        app.reading_direction = if spread.is_rtl() {
            crate::settings::ReadingDirection::Rtl
        } else {
            crate::settings::ReadingDirection::Ltr
        };
        app.settings.still_seek_hover_preview_mode =
            crate::settings::StillSeekHoverPreviewMode::Always;
        let output = preview_output(&mut app, &ctx);
        for idx in [2, 3] {
            let ThumbnailState::Loaded { tex, .. } = &app.thumbnails[idx] else {
                unreachable!()
            };
            let meshes = texture_meshes(&output, tex.id());
            assert_eq!(
                meshes.len(),
                2,
                "page {idx} must have a strip cell and its own preview pane"
            );
            assert_mesh_rotation(meshes.last().unwrap(), rotations[idx]);
        }
    }
}

#[test]
#[ignore = "manual timing probe using only an isolated settings profile"]
fn still_seek_settings_save_measurement() {
    let mut app = still_seek_edge_test_app();
    for idx in 0..crate::settings::MAX_FAVORITES {
        assert!(app.settings.add_favorite(
            format!("measurement-{idx}"),
            PathBuf::from(format!("c:/measurement/{idx}")),
        ));
    }
    assert_eq!(app.settings.favorites.len(), crate::settings::MAX_FAVORITES);
    let mut timings = Vec::new();
    for idx in 0..51 {
        app.settings.set_still_seek_strip_visible(idx % 2 == 0);
        app.settings.set_still_seek_strip_locked(idx % 3 == 0);
        let started = std::time::Instant::now();
        assert!(app.settings.save_checked());
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let first = timings.remove(0);
    timings.sort_by(f64::total_cmp);
    eprintln!(
        "still_seek settings.save: first={first:.3}ms warm n={} median={:.3}ms p95={:.3}ms max={:.3}ms",
        timings.len(),
        timings[25],
        timings[47],
        timings[49]
    );
}

#[test]
fn still_seek_f5_unknown_side_stops_then_worker_rotation_grows_it() {
    for kind in ["image", "zip", "pdf", "converted"] {
        let mut app = still_seek_edge_test_app();
        app.items = (0..10)
            .map(|idx| match kind {
                "zip" | "converted" => GridItem::ZipImage {
                    zip_path: PathBuf::from(if kind == "zip" {
                        "c:/seek/book.zip"
                    } else {
                        "c:/cache/converted.zip"
                    }),
                    entry_name: format!("page-{idx}.png"),
                },
                "pdf" => GridItem::PdfPage {
                    pdf_path: PathBuf::from("c:/seek/book.pdf"),
                    page_num: idx,
                    content_type: None,
                },
                _ => GridItem::Image(PathBuf::from(format!("c:/seek/page-{idx}.png"))),
            })
            .collect();
        if kind == "converted" {
            app.archive_source_override = Some(PathBuf::from("c:/seek/book.rar"));
        }
        let ctx = egui::Context::default();
        load_textures(&mut app, &ctx, &[Rotation::Cw90; 10]);
        let key = app.page_path_key(4).unwrap();
        app.rotation_db
            .as_ref()
            .unwrap()
            .set_key(&key, Rotation::Cw90)
            .unwrap();
        app.remove_rotation_cache_entry_for_reload(4);
        let output = overlay_output(&mut app, &ctx, None);
        let id = |idx| match &app.thumbnails[idx] {
            ThumbnailState::Loaded { tex, .. } => tex.id(),
            _ => unreachable!(),
        };
        assert!(
            !texture_meshes(&output, id(3)).is_empty(),
            "center remains visible"
        );
        assert!(
            !texture_meshes(&output, id(2)).is_empty(),
            "known side keeps growing"
        );
        assert!(
            texture_meshes(&output, id(4)).is_empty(),
            "unknown rotation gets no placeholder cell"
        );
        assert!(
            texture_meshes(&output, id(5)).is_empty(),
            "growth stops at the unknown rotation"
        );
        app.rotation_cache.wait_for_result_for_test();
        let output = overlay_output(&mut app, &ctx, None);
        assert_eq!(
            app.rotation_cache.get(&4),
            Some(&Rotation::Cw90),
            "{kind} uses its canonical page key"
        );
        let ThumbnailState::Loaded { tex, .. } = &app.thumbnails[4] else {
            unreachable!()
        };
        let meshes = texture_meshes(&output, tex.id());
        assert_eq!(meshes.len(), 1);
        assert_mesh_rotation(&meshes[0], Rotation::Cw90);
    }
}

#[test]
fn still_seek_f5_spread_cold_cache_and_invalidation_never_read_sql_on_ui() {
    for spread in [
        crate::settings::SpreadMode::Ltr,
        crate::settings::SpreadMode::Rtl,
    ] {
        let mut app = still_seek_edge_test_app();
        let ctx = egui::Context::default();
        load_textures(&mut app, &ctx, &[Rotation::Cw90; 10]);
        app.spread_mode = spread;
        for idx in 0..10 {
            app.rotation_db
                .as_ref()
                .unwrap()
                .set_key(&app.page_path_key(idx).unwrap(), Rotation::Cw90)
                .unwrap();
        }
        for _ in 0..2 {
            app.clear_rotation_cache_for_reload();
            overlay_output(&mut app, &ctx, None);
            assert!(app.rotation_cache.is_empty());
            app.rotation_cache.wait_for_result_for_test();
            overlay_output(&mut app, &ctx, None);
            for idx in 0..10 {
                assert_eq!(app.rotation_cache.get(&idx), Some(&Rotation::Cw90));
            }
        }
    }
}

#[test]
fn still_seek_f5_completed_old_read_cannot_overwrite_edits_or_invalidated_keys() {
    let mut app = still_seek_edge_test_app();
    let ctx = egui::Context::default();
    for action in ["edit", "remove", "clear", "generation", "close", "hide"] {
        app.rotation_cache.clear();
        app.start_still_seek_rotations(&ctx, &[3]);
        let cancel = app.rotation_cache.pending_cancel_for_test().unwrap();
        app.rotation_cache.wait_for_result_for_test();
        match action {
            "edit" => {
                app.rotation_cache.insert(3, Rotation::Cw180);
            }
            "remove" => app.remove_rotation_cache_entry_for_reload(3),
            "clear" => app.clear_rotation_cache_for_reload(),
            "generation" => app.invalidate_idx_state_and_queues(),
            "close" => app.close_fullscreen(),
            "hide" => app.ensure_still_seek_thumbnail_requests(&ctx, &[]),
            _ => unreachable!(),
        }
        app.poll_still_seek_rotations();
        assert!(
            cancel.load(std::sync::atomic::Ordering::Relaxed),
            "{action}"
        );
        assert_eq!(
            app.rotation_cache.get(&3).copied(),
            (action == "edit").then_some(Rotation::Cw180),
            "{action}"
        );
    }
}

#[test]
fn still_seek_f5_new_request_and_failed_db_have_terminal_results() {
    let mut app = still_seek_edge_test_app();
    let ctx = egui::Context::default();
    app.rotation_cache.clear();
    app.start_still_seek_rotations(&ctx, &[2]);
    let old = app.rotation_cache.pending_cancel_for_test().unwrap();
    app.rotation_cache.wait_for_result_for_test();
    app.start_still_seek_rotations(&ctx, &[3]);
    assert!(old.load(std::sync::atomic::Ordering::Relaxed));
    app.rotation_cache.wait_for_result_for_test();
    app.poll_still_seek_rotations();
    assert!(!app.rotation_cache.contains_key(&2));
    assert_eq!(app.rotation_cache.get(&3), Some(&Rotation::None));

    let mut cache = crate::rotation_cache::RotationCache::default();
    let absent = app.tmp.path().join("absent-rotation.db");
    cache.start_still_seek_rotations(0, vec![(5, "page".into())], absent.clone(), &ctx);
    cache.wait_for_result_for_test();
    cache.poll_still_seek_rotations(0);
    assert_eq!(cache.get(&5), Some(&Rotation::None));
    assert!(
        !absent.exists(),
        "readonly worker must never create a missing DB"
    );
}

#[test]
fn still_seek_f5_rotation_pending_swaps_with_owner_and_close_leaves_sibling_unchanged() {
    let mut app = still_seek_edge_test_app();
    let ctx = egui::Context::default();
    app.build_window_context_for_test(805, |app| {
        app.items = vec![GridItem::Image(PathBuf::from("c:/a.png"))];
        app.fullscreen_idx = Some(0);
        app.start_still_seek_rotations(&ctx, &[0]);
    });
    app.build_window_context_for_test(806, |app| {
        app.items = vec![GridItem::Image(PathBuf::from("c:/b.png"))];
        app.fullscreen_idx = Some(0);
        app.start_still_seek_rotations(&ctx, &[0]);
    });
    let token_a = app
        .with_window_viewer_context(805, |app| {
            app.rotation_cache.wait_for_result_for_test();
            app.rotation_cache.pending_cancel_for_test().unwrap()
        })
        .unwrap();
    let token_b = app
        .with_window_viewer_context(806, |app| {
            app.rotation_cache.pending_cancel_for_test().unwrap()
        })
        .unwrap();
    assert!(!std::sync::Arc::ptr_eq(&token_a, &token_b));
    app.with_window_viewer_context(806, |app| app.close_fullscreen())
        .unwrap();
    assert!(token_b.load(std::sync::atomic::Ordering::Relaxed));
    assert!(!token_a.load(std::sync::atomic::Ordering::Relaxed));
    app.with_window_viewer_context(805, |app| {
        app.poll_still_seek_rotations();
        assert_eq!(app.rotation_cache.get(&0), Some(&Rotation::None));
    })
    .unwrap();
}

#[test]
fn still_seek_f5_rotation_cache_clone_and_drop_do_not_share_pending() {
    let mut app = still_seek_edge_test_app();
    let ctx = egui::Context::default();
    app.rotation_cache.remove(&3);
    app.start_still_seek_rotations(&ctx, &[3]);
    let token = app.rotation_cache.pending_cancel_for_test().unwrap();
    let mut copy = app.rotation_cache.clone();
    assert!(copy.pending_cancel_for_test().is_none());
    copy.clear();
    assert!(!token.load(std::sync::atomic::Ordering::Relaxed));
    app.rotation_cache = Default::default();
    assert!(token.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn still_seek_f5_rotation_reload_keeps_the_displayed_spread_and_release_input() {
    let mut app = still_seek_edge_test_app();
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    let ctx = egui::Context::default();
    let full = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    let track = still_seek_edge_frame(&mut app, &ctx, full, vec![])
        .track
        .unwrap()
        .rect;
    let origin = track.center();
    let moved = egui::pos2(track.left() + track.width() * 0.7, track.center().y);
    let released = egui::pos2(track.right() - 1.0, track.center().y);
    still_seek_edge_frame(
        &mut app,
        &ctx,
        full,
        vec![egui::Event::PointerMoved(origin)],
    );
    still_seek_edge_frame(
        &mut app,
        &ctx,
        full,
        vec![still_seek_pointer_button_event(origin, true)],
    );
    still_seek_edge_frame(&mut app, &ctx, full, vec![egui::Event::PointerMoved(moved)]);
    app.clear_rotation_cache_for_reload();
    let before = crate::rotation_db::read_operations_for_test();
    let result = still_seek_edge_frame(
        &mut app,
        &ctx,
        full,
        vec![
            egui::Event::PointerMoved(released),
            still_seek_pointer_button_event(released, false),
        ],
    );
    assert_eq!(crate::rotation_db::read_operations_for_test(), before);
    assert!(
        result.track.is_some(),
        "a metadata reload must not remove the existing track"
    );
    assert_eq!(
        result.target,
        Some(8),
        "the release resolves against the displayed spread units"
    );
    assert_eq!(app.fs_seek_gesture, StillSeekGesture::Idle);
}

#[test]
fn still_seek_f6_real_overlay_rotation_snapshots() {
    let mut snapshots = egui_kittest::SnapshotResults::new();
    for (name, rotation, spread) in [
        (
            "still_seek_real_overlay_rotation_0",
            Rotation::None,
            crate::settings::SpreadMode::Single,
        ),
        (
            "still_seek_real_overlay_rotation_90",
            Rotation::Cw90,
            crate::settings::SpreadMode::Single,
        ),
        (
            "still_seek_real_overlay_rotation_180",
            Rotation::Cw180,
            crate::settings::SpreadMode::Single,
        ),
        (
            "still_seek_real_overlay_rotation_270",
            Rotation::Cw270,
            crate::settings::SpreadMode::Single,
        ),
        (
            "still_seek_real_overlay_rotation_spread_ltr",
            Rotation::Cw90,
            crate::settings::SpreadMode::Ltr,
        ),
        (
            "still_seek_real_overlay_rotation_spread_rtl",
            Rotation::Cw90,
            crate::settings::SpreadMode::Rtl,
        ),
    ] {
        let mut app = still_seek_edge_test_app();
        app.spread_mode = spread;
        app.reading_direction = if spread.is_rtl() {
            crate::settings::ReadingDirection::Rtl
        } else {
            crate::settings::ReadingDirection::Ltr
        };
        app.settings.still_seek_hover_preview_mode =
            crate::settings::StillSeekHoverPreviewMode::Always;
        let mut rotations = [rotation; 10];
        if spread.is_spread() {
            rotations[2] = Rotation::Cw270;
        }
        for (idx, &rot) in rotations.iter().enumerate() {
            app.rotation_cache.insert(idx, rot);
        }
        let mut initialized = false;
        let hover = std::cell::Cell::new(egui::Pos2::ZERO);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(640.0, 480.0))
            .build(|ctx| {
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                if !initialized {
                    crate::ui_fonts::configure_fonts(ctx);
                    load_textures(&mut app, ctx, &rotations);
                    initialized = true;
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        let full = ctx.content_rect();
                        let media = app.fullscreen_media_rect(full, 3, false);
                        let pages = if spread.is_spread() {
                            if spread.is_rtl() {
                                vec![3, 2]
                            } else {
                                vec![2, 3]
                            }
                        } else {
                            vec![3]
                        };
                        for (pane, &idx) in pages.iter().enumerate() {
                            let ThumbnailState::Loaded { tex, .. } = &app.thumbnails[idx] else {
                                unreachable!()
                            };
                            let bounds = egui::Rect::from_min_size(
                                media.min
                                    + egui::vec2(
                                        media.width() / pages.len() as f32 * pane as f32,
                                        0.0,
                                    ),
                                egui::vec2(media.width() / pages.len() as f32, media.height()),
                            );
                            crate::app::draw_rotated_image(
                                ui.painter(),
                                tex.id(),
                                fit_texture_rect(
                                    rotated_display_size(tex.size_vec2(), rotations[idx]),
                                    bounds.shrink(8.0),
                                ),
                                rotations[idx],
                            );
                        }
                        app.draw_fullscreen_seek_overlay(ui, ctx, full, 3, false, false);
                        let strip = app
                            .still_seek_geometry_for_idx(3, false)
                            .strip_rect(full)
                            .unwrap();
                        let lock =
                            crate::video::seek_strip_layout::seek_strip_lock_button_rect(strip);
                        hover.set(egui::pos2(
                            (strip.left() + lock.left()) * 0.5,
                            strip.center().y,
                        ));
                    });
            });
        harness.run();
        harness.hover_at(hover.get());
        harness.run();
        harness.snapshot(name);
        snapshots.extend_harness(&mut harness);
    }
    snapshots.unwrap();
}
