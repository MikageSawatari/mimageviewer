//! Capture the production MainWindow renderer, including paint order after projection.
#![cfg(all(test, windows))]
use super::*;
use crate::book_composition::{PageOverride, PageSide, PresentationDecision};

struct CapturedFrame {
    shapes: Vec<egui::epaint::ClippedShape>,
    meshes: Vec<(egui::Rect, egui::Mesh)>,
}

fn capture(
    app: &mut App,
    ctx: &egui::Context,
    time: f64,
    before_render: impl FnOnce(&mut App, &egui::Context),
) -> CapturedFrame {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 800.0),
        )),
        time: Some(time),
        ..Default::default()
    };
    let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
    viewport.focused = Some(true);
    viewport.native_pixels_per_point = Some(1.5);
    let mut before_render = Some(before_render);
    let output = crate::test_ui::headless_paint_output(ctx.run_pass(input, |ctx| {
        // A real input is consumed once; a sizing/discard pass must not issue
        // the navigation handler again merely because egui re-runs painting.
        if let Some(before_render) = before_render.take() {
            before_render(app, ctx);
        }
        // Deliberately do not use page_turn_submission_tests::draw_current:
        // a later normal-page draw or background can hide a valid curl receipt.
        ctx.root_ui(|root_ui| app.render_fullscreen_viewport(root_ui));
    }));
    let meshes = ctx
        .tessellate(output.shapes.clone(), output.pixels_per_point)
        .into_iter()
        .filter_map(|primitive| match primitive.primitive {
            egui::epaint::Primitive::Mesh(mesh) => Some((primitive.clip_rect, mesh)),
            egui::epaint::Primitive::Callback(_) => None,
        })
        .collect();
    let frame = CapturedFrame {
        shapes: output.shapes.clone(),
        meshes,
    };
    output.drop_without_applying_deltas();
    frame
}

fn bounds(mesh: &egui::Mesh) -> egui::Rect {
    mesh.vertices.iter().fold(egui::Rect::NOTHING, |r, v| {
        r.union(egui::Rect::from_min_max(v.pos, v.pos))
    })
}

fn page_rect(unit: &PaintedUnit, idx: usize) -> (egui::TextureId, egui::Rect) {
    let page = unit
        .pages
        .iter()
        .find(|page| page.occurrence.idx == idx)
        .unwrap();
    let canonical = canonical_unit(unit).unwrap();
    let id = page.resource.paint_texture_id();
    let rect = canonical
        .pages
        .iter()
        .find_map(|page| match page.material {
            crate::page_turn_animation::PageMaterial::Texture { texture_id, .. }
                if texture_id == id =>
            {
                Some(page.rect)
            }
            _ => None,
        })
        .unwrap();
    (id, rect)
}

fn assert_rect_near(actual: egui::Rect, expected: egui::Rect) {
    assert!(
        (actual.min - expected.min).length() < 2.0 && (actual.max - expected.max).length() < 2.0,
        "submitted rectangle {actual:?}, expected {expected:?}"
    );
}

fn page_meshes(frame: &CapturedFrame, id: egui::TextureId) -> Vec<(usize, &egui::Mesh)> {
    frame
        .meshes
        .iter()
        .enumerate()
        .filter(|(_, (_, mesh))| mesh.texture_id == id)
        .map(|(order, (_, mesh))| (order, mesh))
        .collect()
}

fn assert_static_page(frame: &CapturedFrame, id: egui::TextureId, rect: egui::Rect) -> usize {
    let meshes = page_meshes(frame, id);
    assert_eq!(meshes.len(), 1, "one retained static page {id:?}");
    assert!(
        meshes[0].1.vertices.iter().all(|v| v.color.a() == 255),
        "retained static page must not fade"
    );
    assert_rect_near(bounds(meshes[0].1), rect);
    meshes[0].0
}

// The fixture textures are solid, so an opaque triangle's texture identity is
// sufficient to determine the last visible page at a point. Partial shadow
// triangles do not conceal the opaque surface beneath them.
#[derive(Clone, Copy, Debug, PartialEq)]
enum OpaqueSurface {
    Page(egui::TextureId),
    Black,
    Other,
}

fn opaque_at(frame: &CapturedFrame, point: egui::Pos2) -> Option<OpaqueSurface> {
    let mut last = None;
    for (clip, mesh) in &frame.meshes {
        if !clip.contains(point) {
            continue;
        }
        for triangle in mesh.indices.chunks_exact(3) {
            let vertices =
                [triangle[0], triangle[1], triangle[2]].map(|i| mesh.vertices[i as usize]);
            let [a, b, c] = vertices.map(|v| v.pos);
            let cross = |u: egui::Vec2, v: egui::Vec2| u.x * v.y - u.y * v.x;
            let area = cross(b - a, c - a);
            if area.abs() < 0.0001 {
                continue;
            }
            let weights = [
                cross(b - point, c - point),
                cross(c - point, a - point),
                cross(a - point, b - point),
            ]
            .map(|w| w / area);
            if weights.iter().any(|w| *w < -0.0001) {
                continue;
            }
            if vertices.iter().all(|v| v.color.a() == 255) {
                last = Some(if mesh.texture_id != egui::TextureId::default() {
                    OpaqueSurface::Page(mesh.texture_id)
                } else if vertices
                    .iter()
                    .all(|v| v.color.r() == 0 && v.color.g() == 0 && v.color.b() == 0)
                {
                    OpaqueSurface::Black
                } else {
                    OpaqueSurface::Other
                });
            }
        }
    }
    last
}

fn visible_samples(frame: &CapturedFrame, rect: egui::Rect, wanted: OpaqueSurface) -> usize {
    // Include thin outer strips: perspective can leave less than 5% of the
    // stationary page visible near landing. A 5%-only grid misses it entirely.
    [0.01, 0.02]
        .into_iter()
        .chain((1..20).map(|x| x as f32 / 20.0))
        .chain([0.98, 0.99])
        .flat_map(|x| (1..10).map(move |y| (x, y)))
        .filter(|(x, y)| {
            opaque_at(
                frame,
                egui::pos2(
                    rect.left() + rect.width() * *x,
                    rect.top() + rect.height() * *y as f32 / 10.0,
                ),
            ) == Some(wanted)
        })
        .count()
}

fn shape_mesh_count(shape: &egui::Shape) -> usize {
    match shape {
        egui::Shape::Mesh(_) => 1,
        egui::Shape::Vec(shapes) => shapes.iter().map(shape_mesh_count).sum(),
        _ => 0,
    }
}

#[test]
fn page_turn_mainwindow_submits_visible_vacant_leaf_both_directions() {
    let _serial = crate::key_input::lock_test_input();
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        crate::key_input::clear_test_frame();
        let ctx = egui::Context::default();
        let directives = [
            (10, PageOverride::Alone),
            (11, PageOverride::Alone),
            (
                15,
                PageOverride::BlankOpposite {
                    page_side: PageSide::Leading,
                },
            ),
            (
                16,
                PageOverride::BlankOpposite {
                    page_side: PageSide::Trailing,
                },
            ),
            (17, PageOverride::Alone),
            (
                18,
                PageOverride::BlankOpposite {
                    page_side: PageSide::Trailing,
                },
            ),
        ];
        let (mut app, _temp, owner) =
            super::permission_service_regression_tests::book_fixture_with_pages(
                &ctx,
                false,
                true,
                mode,
                40,
                &[4, 2],
                &directives,
            );
        app.viewer_presentation = crate::app::ViewerPresentation::MainWindow;
        app.native_video_in_window_active = true;
        app.settings.video_in_window_mode = true;
        app.settings.spread_page_gap_px = 4;
        app.settings.final_cover_spread_enabled = false;
        app.settings.fullscreen_navigator_visible = false;
        app.settings.fullscreen_top_bar_locked = false;
        app.settings.still_seek_strip_visible = false;
        app.settings
            .set_still_bottom_lock(crate::settings::BottomBarLock::None);
        app.fs_info_panel.locked = false;
        for idx in 0..app.items.len() {
            let pixels = Arc::new(egui::ColorImage::filled(
                [932, 1320],
                egui::Color32::from_rgb(30 + idx as u8 * 5, 60 + idx as u8 * 3, 190),
            ));
            let tex = ctx.load_texture(
                format!("mainwindow-vacant-{idx}"),
                pixels.clone(),
                egui::TextureOptions::LINEAR,
            );
            app.fs_cache.insert(
                idx,
                FsCacheEntry::Static {
                    pdf_rerender_failures: Vec::new(),
                    pdf_metadata: None,
                    tex,
                    pixels,
                    source_dims: Some([932, 1320]),
                    load_seq: 1,
                    animation: crate::fs_animation::StaticAnimationState::Still,
                },
            );
            app.record_page_dims_for_spread(idx, (932, 1320));
            app.record_page_layout_dims_for_spread(idx, (932, 1320));
            app.rotation_cache
                .insert(idx, crate::rotation_db::Rotation::None);
            let _ = app.ensure_final_composite_texture(&ctx, idx).unwrap();
        }
        // Match the runtime: completed Retained material survives without originals.
        app.fs_cache.clear();
        app.edit_result_cache.clear();
        // Both units have already been displayed in the reported reproduction.
        // Warm the actual target preparation as well as its cached material.
        let mut time = 0.0;
        for anchor in [8, 6] {
            app.fullscreen_idx = Some(anchor);
            let demand = app
                .spread_display_composition_for_anchor(anchor)
                .presentation_demand(app.spread_mode, app.fullscreen_page_slice);
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                app.retain_book_presentation_demand(&demand);
                if matches!(
                    app.book_presentation_current(&demand),
                    PresentationDecision::Allowed(_)
                ) {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            for _ in 0..2 {
                capture(&mut app, &ctx, time, |_, _| {});
                time += 0.01;
            }
        }
        let initial = capture(&mut app, &ctx, time, |_, _| {});
        let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
            panic!("actual MainWindow renderer must establish outgoing flat");
        };
        let (page9, static9_rect) = page_rect(unit, 6);
        let (page10, turn10_rect) = page_rect(unit, 7);
        assert!(if mode.is_rtl() {
            turn10_rect.right() <= static9_rect.left()
        } else {
            static9_rect.right() <= turn10_rect.left()
        });
        assert_static_page(&initial, page9, static9_rect);
        assert_static_page(&initial, page10, turn10_rect);
        assert_eq!(
            opaque_at(&initial, static9_rect.center()),
            Some(OpaqueSurface::Page(page9))
        );
        assert_eq!(
            opaque_at(&initial, turn10_rect.center()),
            Some(OpaqueSurface::Page(page10))
        );

        for target in [8, 6] {
            let forward = target == 8;
            take_test_events();
            let operation = ViewerOperationId::Pointer {
                batch: next_operation_batch(),
                source: egui::ViewportId::ROOT,
                ordinal: 0,
            };
            time += 0.01;
            let mut admission = None;
            capture(&mut app, &ctx, time, |app, ctx| {
                let from = app.fullscreen_idx.unwrap();
                app.begin_page_turn_viewer_frame(ctx, true, None);
                app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "wheel");
                app.handle_fs_navigation_with_operation(
                    ctx,
                    false,
                    false,
                    None,
                    None,
                    None,
                    FsPageNav::Target(target),
                    None,
                    from,
                    Some(operation),
                );
                assert_eq!(app.fullscreen_idx, Some(target));
                if let PresentationState::Offered { outgoing, offer } =
                    &app.page_turn_presentation.state
                {
                    admission = Some((
                        outgoing.clone(),
                        offer.ready_sources.0.clone(),
                        offer.fact.destination,
                        offer.accepted_at,
                    ));
                }
            });
            let PresentationState::Turning {
                offer,
                incoming,
                outgoing,
                ..
            } = &app.page_turn_presentation.state
            else {
                let mismatch = admission.as_ref().and_then(|(old, sources, destination, accepted_at)| {
                let PresentationState::Flat(current) = &app.page_turn_presentation.state else { return None; };
                Some(serde_json::json!({
                    "accepted":old.evidence.was_accepted(),
                    "destination":*destination == current.destination,
                    "items":old.items == current.items,
                    "mode":old.mode == current.mode,
                    "reading_rtl":old.reading_rtl == current.reading_rtl,
                    "slice":old.slice == current.slice,
                    "layout":Arc::ptr_eq(&old.layout,&current.layout),
                    "elapsed_ms":accepted_at.elapsed().as_millis(),
                    "sources":sources.iter().map(|s|format!("{:?} {:?} {:?}",s.occurrence,s.resource.source_texture_id(),s.resource.source_generation())).collect::<Vec<_>>(),
                    "actual":current.pages.iter().map(|p|format!("{:?} {:?} {:?}",p.occurrence,p.resource.source_texture_id(),p.resource.source_generation())).collect::<Vec<_>>(),
                    "content":sources.iter().zip(&current.pages).map(|(s,p)|s.content == app.page_turn_content(p.occurrence.idx)).collect::<Vec<_>>()
                }))
            });
                panic!(
                    "actual MainWindow renderer must start turn: {:?}; mismatch {mismatch:?}",
                    take_test_events()
                );
            };
            let tracker = offer.submission.clone();
            let (page11, under11_rect) = page_rect(if forward { incoming } else { outgoing }, 8);
            assert_ne!(page9, page10);
            assert_ne!(page10, page11);
            let mut black_counts = Vec::new();
            for progress in [0.1, 0.25, 0.6, 0.75] {
                let PresentationState::Turning { clock, offer, .. } =
                    &mut app.page_turn_presentation.state
                else {
                    panic!("effect must remain active at {progress}");
                };
                *clock = MotionClock::Running {
                    start_at: Instant::now() - offer.duration.mul_f32(progress),
                };
                time += 0.01;
                let frame = capture(&mut app, &ctx, time, |_, _| {});
                assert_eq!(tracker.status(), SubmissionStatus::Started);
                assert!(
                    frame
                        .shapes
                        .iter()
                        .map(|s| shape_mesh_count(&s.shape))
                        .sum::<usize>()
                        > 0,
                    "production output must contain the submitted projection meshes"
                );
                let static_id = if forward { page9 } else { page11 };
                let static_rect = if forward { static9_rect } else { under11_rect };
                let static_order = assert_static_page(&frame, static_id, static_rect);
                let image_face_visible = if forward {
                    progress < 0.5
                } else {
                    progress > 0.5
                };
                let leaf = page_meshes(&frame, page10);
                if image_face_visible {
                    assert_eq!(leaf.len(), 1, "one visible curved page10 at {progress}");
                    let (order, mesh) = leaf[0];
                    assert!(
                        order > static_order,
                        "leaf must paint over the retained static page"
                    );
                    assert!(
                        mesh.vertices.len() > 4 && mesh.indices.len() > 6,
                        "actual submitted page10 must be a curved mesh, not a flat quad"
                    );
                    assert!(
                        bounds(mesh).width() < turn10_rect.width() * 0.99,
                        "turning page must contract before/after the edge-on pose"
                    );
                    assert!(
                        visible_samples(&frame, bounds(mesh), OpaqueSurface::Page(page10)) > 0,
                        "a later flat target/background must not obscure the submitted leaf"
                    );
                } else {
                    assert!(
                        leaf.is_empty(),
                        "hidden image face must not be drawn as a flat overlay"
                    );
                }
                if forward && progress < 0.5 {
                    let under = page_meshes(&frame, page11);
                    assert_eq!(under.len(), 1, "new11 must be present below old10");
                    assert!(
                        under[0].0 < leaf[0].0,
                        "new11 must paint before the turning page10"
                    );
                    assert_eq!(
                        opaque_at(&frame, static9_rect.center()),
                        Some(OpaqueSurface::Page(page9)),
                        "old9 must remain visible before the black back crosses the spine"
                    );
                }
                let black = visible_samples(&frame, static9_rect, OpaqueSurface::Black);
                black_counts.push(black);
                if forward && progress > 0.5 {
                    assert!(black > 0, "black vacant back must visibly cover old9");
                    assert!(
                        visible_samples(&frame, static9_rect, OpaqueSurface::Page(page9)) > 0,
                        "old9 must remain visible outside the growing black leaf: p={progress}, old9={static9_rect:?}, old10={turn10_rect:?}, new11={under11_rect:?}, black={black_counts:?}, output={:?}",
                        output_paint_diagnostics(&frame.shapes)
                    );
                }
                if !forward && progress < 0.5 {
                    assert!(
                        black > 0,
                        "reverse starts with a visible black vacant front"
                    );
                    if progress >= 0.25 {
                        assert!(
                            visible_samples(&frame, static9_rect, OpaqueSurface::Page(page9)) > 0,
                            "departing vacant front must reveal new9 beneath it"
                        );
                    }
                }
            }
            if forward {
                assert!(
                    black_counts[3] > black_counts[2],
                    "black back must cover progressively more old9: {black_counts:?}"
                );
            } else {
                assert!(
                    black_counts[0] > black_counts[1] && black_counts[1] > black_counts[2],
                    "black front must uncover new9 progressively: {black_counts:?}"
                );
            }
            let PresentationState::Turning { clock, offer, .. } =
                &mut app.page_turn_presentation.state
            else {
                panic!("turn must survive middle frames");
            };
            *clock = MotionClock::Running {
                start_at: Instant::now() - offer.duration - Duration::from_millis(1),
            };
            time += 0.01;
            capture(&mut app, &ctx, time, |_, _| {});
            time += 0.01;
            let final_frame = capture(&mut app, &ctx, time, |_, _| {});
            assert_eq!(tracker.status(), SubmissionStatus::Finished);
            assert_static_page(
                &final_frame,
                if forward { page11 } else { page10 },
                if forward { under11_rect } else { turn10_rect },
            );
            if forward {
                assert!(page_meshes(&final_frame, page9).is_empty());
                assert!(page_meshes(&final_frame, page10).is_empty());
                assert_eq!(
                    opaque_at(&final_frame, static9_rect.center()),
                    Some(OpaqueSurface::Black)
                );
            } else {
                assert_static_page(&final_frame, page9, static9_rect);
                assert!(page_meshes(&final_frame, page11).is_empty());
            }
            let events = take_test_events();
            for kind in ["start", "finish"] {
                assert!(
                    events
                        .iter()
                        .any(|(fact, actual, _)| fact.operation_id == operation && *actual == kind),
                    "missing {kind}: {events:?}"
                );
            }
            assert!(
                !events
                    .iter()
                    .any(|(_, kind, _)| matches!(*kind, "skip" | "cancel")),
                "{events:?}"
            );
        }
        owner.shutdown_and_join();
    }
}
