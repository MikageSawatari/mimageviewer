use super::*;
use crate::app::PollPrefetchOrigin;
use crate::app::fs_load_test_probe::{Probe, Request};
use crate::fs_animation::FsLoadResult;
use crate::grid_item::ThumbnailState;

fn book(
    ctx: &egui::Context,
    pdf: bool,
    flow: ReadingFlow,
    mode: SpreadMode,
    animation_enabled: bool,
) -> crate::app::AppTestEnvForTest {
    let mut app = crate::app::setup_app_for_test();
    let path = app
        .tmp
        .path()
        .join(if pdf { "book.pdf" } else { "book.zip" });
    app.current_folder = Some(path.clone());
    app.items = (0..24)
        .map(|idx| {
            if pdf {
                GridItem::PdfPage {
                    pdf_path: path.clone(),
                    page_num: idx,
                    content_type: Some(crate::pdf_loader::PdfPageContentType::Vector),
                }
            } else {
                GridItem::ZipImage {
                    zip_path: path.clone(),
                    entry_name: format!("{idx:03}.png"),
                }
            }
        })
        .collect();
    app.visible_indices = (0..24).collect();
    app.details_order = app.visible_indices.clone();
    app.items_generation += 1;
    app.settings.page_turn_animation_enabled = animation_enabled;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Page;
    app.reading_flow = flow;
    app.spread_mode = mode;
    app.reading_direction = mode
        .reading_direction()
        .unwrap_or(crate::settings::ReadingDirection::Ltr);
    app.fullscreen_idx = Some(3);
    app.fs_zoom = 0.55;
    for idx in 0..24 {
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
        app.record_page_dims_for_spread(idx, (600, 900));
        if pdf {
            app.record_page_layout_dims_for_spread(idx, (600_000, 900_000));
        }
    }
    app.thumbnails = (0..24)
        .map(|idx| {
            ThumbnailState::loaded_raster(
                ctx.load_texture(
                    format!("catalog-{idx}"),
                    egui::ColorImage::filled([374, 561], egui::Color32::GRAY),
                    egui::TextureOptions::LINEAR,
                ),
                crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                    evaluated_display_px: 561,
                },
                false,
                561,
                crate::source_coverage::SourceCoverage::FullSource,
                Some((600, 900)),
                Some(if pdf { (600_000, 900_000) } else { (600, 900) }),
            )
        })
        .collect();
    if pdf {
        use crate::material_display_basis::{MaterialDisplayBasis, MaterialLegacyBasis};
        let layout_size = egui::vec2(600_000.0, 900_000.0);
        let legacy = MaterialLegacyBasis::SourceLayout {
            basis: crate::fullscreen_specified_scale::LegacySourceLayoutBasis {
                layout_size,
                reference_size: egui::vec2(374.0, 561.0),
            },
        };
        for thumb in &mut app.thumbnails {
            if let ThumbnailState::Loaded { display_basis, .. } = thumb {
                *display_basis = MaterialDisplayBasis::new(
                    legacy,
                    legacy,
                    legacy,
                    crate::fullscreen_specified_scale::FullscreenFitBasis::from_pdf(
                        layout_size,
                        crate::pdf_loader::PdfPageContentType::Vector,
                    )
                    .ok_or(
                        crate::material_display_basis::MaterialBasisUnavailable::InvalidDimensions,
                    ),
                );
            }
        }
    }
    app
}

fn draw(app: &mut App, ctx: &egui::Context) -> FsNavigatorTextureSources {
    draw_with_delay(app, ctx).0
}

fn draw_with_delay(
    app: &mut App,
    ctx: &egui::Context,
) -> (FsNavigatorTextureSources, std::time::Duration) {
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2400.0, 1600.0));
    let mut sources = FsNavigatorTextureSources::default();
    let idx = app.fullscreen_idx.unwrap();
    let output = crate::test_ui::headless_paint_output(ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(rect),
            max_texture_side: Some(8192),
            time: Some(ctx.cumulative_frame_nr() as f64),
            ..Default::default()
        },
        |ctx| {
            ctx.root_ui(|root| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(root, |ui| {
                        sources = app.draw_fs_continuous_reading(
                            ui,
                            ctx,
                            rect,
                            idx,
                            false,
                            FsPageTurnDecision::normal(),
                        );
                    })
            });
        },
    ));
    let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
    output.drop_without_applying_deltas();
    (sources, delay)
}

fn complete_requests(app: &mut App, ctx: &egui::Context, probe: &Probe, pdf: bool) -> Vec<usize> {
    complete_request_batch(app, ctx, probe, pdf, probe.take_requests())
}

fn pdf_request_long_edge(request: &Request) -> u32 {
    request.target_px.unwrap_or_else(|| {
        request
            .pdf_target
            .map(|target| {
                crate::pdf_loader::display_render_long_edge(
                    target,
                    600.0,
                    900.0,
                    false,
                    crate::pdf_loader::PdfPageContentType::Vector,
                )
            })
            .unwrap_or(900)
    })
}

fn complete_request_batch(
    app: &mut App,
    ctx: &egui::Context,
    probe: &Probe,
    pdf: bool,
    requests: Vec<Request>,
) -> Vec<usize> {
    let indices = requests.iter().map(|r| r.idx).collect::<Vec<_>>();
    // A real paged pass can supersede a request before dispatch completion.
    // Probe owns only the newest sender per page, just as fs_pending does.
    let latest = requests
        .iter()
        .enumerate()
        .map(|(pos, r)| (r.idx, pos))
        .collect::<HashMap<_, _>>();
    for (pos, request) in requests.into_iter().enumerate() {
        if latest[&request.idx] != pos {
            continue;
        }
        let dims = if pdf {
            let long = pdf_request_long_edge(&request);
            [(long as f32 * 2.0 / 3.0).round() as usize, long as usize]
        } else {
            [600, 900]
        };
        let ci = egui::ColorImage::filled(dims, egui::Color32::WHITE);
        let result = if pdf {
            FsLoadResult::StaticPdf {
                ci,
                source_dims: dims,
                page_size_points: crate::pdf_loader::PdfPageSizePoints {
                    width: 600.0,
                    height: 900.0,
                },
                content_type: crate::pdf_loader::PdfPageContentType::Vector,
            }
        } else {
            FsLoadResult::Static {
                ci,
                source_dims: [600, 900],
                animation: crate::fs_animation::StaticAnimationState::Still,
            }
        };
        probe.complete(request.idx, result);
    }
    app.poll_prefetch(ctx, PollPrefetchOrigin::TopLevel);
    indices
}

#[test]
fn section1674_fresh_pdf_continuous_registers_display_requests() {
    let ctx = egui::Context::default();
    let mut app = book(&ctx, true, ReadingFlow::Horizontal, SpreadMode::Rtl, true);
    let probe = Probe::start();
    let sources = draw(&mut app, &ctx);
    assert!(sources.pages.len() >= 2);
    assert!(
        app.fs_pdf_display_target.is_some(),
        "continuous PDF never registered its drawing target"
    );
    assert!(
        !probe.take_requests().is_empty(),
        "visible PDF load was silently skipped"
    );
}

#[test]
fn section1674_visible_pages_converge_to_full_resolution_for_pdf_and_zip() {
    for pdf in [true, false] {
        for flow in [ReadingFlow::Vertical, ReadingFlow::Horizontal] {
            for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
                let mut baseline = None;
                for animation_enabled in [true, false] {
                    let case = format!("pdf={pdf} flow={flow:?} animation={animation_enabled}");
                    let ctx = egui::Context::default();
                    let mut app = book(&ctx, pdf, flow, mode, animation_enabled);
                    let probe = Probe::start();
                    let mut requested = HashSet::new();
                    let initial = draw(&mut app, &ctx);
                    assert!(
                        !pdf || app.fs_pdf_display_target.is_some(),
                        "continuous PDF target missing: {case} mode={mode:?}"
                    );
                    assert!(
                        initial.pages.len() >= 2,
                        "fixture must show multiple pages: {case} mode={mode:?}"
                    );
                    let mut sources = initial;
                    for _ in 0..12 {
                        requested.extend(complete_requests(&mut app, &ctx, &probe, pdf));
                        sources = draw(&mut app, &ctx);
                    }
                    let mut result = Vec::new();
                    for page in sources.pages {
                        let idx = page.occurrence.idx;
                        assert!(
                            requested.contains(&idx),
                            "no source request for visible idx={idx}: {case} mode={mode:?}"
                        );
                        let texture = page.resource.source_texture().unwrap();
                        result.push((idx, texture.size()));
                        assert!(
                            texture.size()[1] > 561,
                            "catalog sampler still painted for idx={idx}: {case} mode={mode:?}"
                        );
                    }
                    assert!(
                        requested.len() < 24,
                        "offscreen loading must remain bounded: {case} mode={mode:?}"
                    );
                    result.sort_unstable();
                    if let Some(baseline) = &baseline {
                        assert_eq!(
                            &result, baseline,
                            "animation changed visible quality: {case} mode={mode:?}"
                        );
                    } else {
                        baseline = Some(result);
                    }
                }
            }
        }
    }
}

#[test]
fn section1674_visible_neighbors_survive_while_current_is_loading() {
    let ctx = egui::Context::default();
    let mut app = book(
        &ctx,
        false,
        ReadingFlow::Horizontal,
        SpreadMode::Single,
        true,
    );
    let probe = Probe::start();
    let sources = draw(&mut app, &ctx);
    let visible = sources
        .pages
        .iter()
        .map(|p| p.occurrence.idx)
        .collect::<HashSet<_>>();
    assert!(visible.len() >= 2);
    let requests = probe.take_requests();
    for idx in &visible {
        assert!(
            requests.iter().any(|r| r.idx == *idx
                && r.priority == crate::fs_page_load_scheduler::FsPageLoadPriority::High),
            "visible idx={idx} must share display priority"
        );
    }
    let cancel = visible
        .iter()
        .map(|idx| (*idx, app.fs_pending.get(idx).unwrap().cancel.clone()))
        .collect::<Vec<_>>();
    for _ in 0..3 {
        draw(&mut app, &ctx);
    }
    assert!(
        probe.take_requests().is_empty(),
        "unchanged demand must join the same pending"
    );
    for (idx, cancel) in cancel {
        assert!(
            !cancel.load(std::sync::atomic::Ordering::Relaxed),
            "visible neighbor {idx} was cancelled"
        );
    }
}

#[test]
fn section1674_grid_double_click_direct_open_and_scroll_share_display_loading() {
    for pdf in [true, false] {
        for flow in [ReadingFlow::Vertical, ReadingFlow::Horizontal] {
            for entry in 0..3 {
                let mut baseline = None;
                for animation_enabled in [true, false] {
                    let case = format!("pdf={pdf} flow={flow:?} animation={animation_enabled}");
                    let ctx = egui::Context::default();
                    let mut app = book(&ctx, pdf, flow, SpreadMode::Single, animation_enabled);
                    let probe = Probe::start();
                    app.fullscreen_idx = None;
                    if entry == 0 {
                        // The same synthetic pointer handler used by the list-order tests.
                        assert!(
                            crate::ui_main::ratingnav649_pointer_open_for_test(&mut app, 3, false)
                                .is_none(),
                            "list pointer open failed: {case} entry={entry}"
                        );
                    } else if entry == 1 {
                        // Source enumeration is a fixture; its actual direct-open
                        // continuation and fullscreen admission remain production code.
                        app.complete_deferred_book_composition_open(crate::app::DeferredFsReopen {
                            history_trigger: crate::app::HistoryTrigger::UserChosen,
                            resume_slideshow: false,
                            target: crate::app::DeferredFsTarget::None,
                            resume_to_last_page: false,
                            from_explicit_open: true,
                            preserve_after_password_prompt: true,
                        });
                    } else {
                        app.open_fullscreen(3, crate::app::HistoryTrigger::UserChosen);
                    }
                    assert_eq!(
                        app.fullscreen_idx,
                        Some(if entry == 1 { 0 } else { 3 }),
                        "{case} entry={entry}"
                    );
                    assert_eq!(app.reading_flow, flow, "{case} entry={entry}");
                    app.fs_zoom = 0.55;
                    let mut requested = HashSet::new();
                    for _ in 0..12 {
                        draw(&mut app, &ctx);
                        assert!(
                            !pdf || app.fs_pdf_display_target.is_some(),
                            "continuous PDF target missing after open: {case} entry={entry}"
                        );
                        requested.extend(complete_requests(&mut app, &ctx, &probe, pdf));
                    }
                    if entry == 2 {
                        let current_idx = app.fullscreen_idx.unwrap();
                        let (units, pos) =
                            app.continuous_reading_units_and_pos(current_idx).unwrap();
                        let rect =
                            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2400.0, 1600.0));
                        let (_, _, offsets, _) = app
                            .continuous_reading_layout(&ctx, rect, &units, pos, false)
                            .unwrap();
                        app.fs_vertical_scroll = offsets[(pos + 8).min(units.len() - 1)];
                    } else {
                        // Common page-navigation admission, after the initial open has settled.
                        app.open_fullscreen(15, crate::app::HistoryTrigger::UserChosen);
                        app.fs_zoom = 0.55;
                    }
                    for _ in 0..12 {
                        draw(&mut app, &ctx);
                        requested.extend(complete_requests(&mut app, &ctx, &probe, pdf));
                    }
                    let sources = draw(&mut app, &ctx);
                    assert!(sources.pages.len() >= 2, "{case} entry={entry}");
                    assert!(app.fullscreen_idx.unwrap() > 3, "{case} entry={entry}");
                    let mut result = Vec::new();
                    for page in sources.pages {
                        assert!(
                            requested.contains(&page.occurrence.idx),
                            "no visible request: {case} entry={entry} idx={}",
                            page.occurrence.idx
                        );
                        let size = page.resource.source_texture().unwrap().size();
                        assert!(
                            size[1] > 561,
                            "catalog sampler still painted: {case} entry={entry} idx={}",
                            page.occurrence.idx
                        );
                        result.push((page.occurrence.idx, size));
                    }
                    result.sort_unstable();
                    let result = (app.fullscreen_idx, result);
                    if let Some(baseline) = &baseline {
                        assert_eq!(
                            &result, baseline,
                            "animation changed open/scroll result: {case} entry={entry}"
                        );
                    } else {
                        baseline = Some(result);
                    }
                }
            }
        }
    }
}

// Use the real paged consumer to seed its PDF target and cache. Embedded rendering
// runs entirely in egui's headless pass; no native window or application is opened.
#[cfg(windows)]
fn draw_paged(app: &mut App, ctx: &egui::Context) {
    app.native_video_in_window_active = true;
    crate::test_ui::headless_paint_output(ctx.run_pass(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(2400.0, 1600.0),
            )),
            max_texture_side: Some(8192),
            time: Some(ctx.cumulative_frame_nr() as f64),
            ..Default::default()
        },
        |ctx| ctx.root_ui(|root| app.render_fullscreen_viewport(root)),
    ))
    .drop_without_applying_deltas();
}

#[cfg(windows)]
#[test]
fn section1674_paged_continuous_round_trip_refreshes_pdf_geometry_and_visible_quality() {
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        for flow in [ReadingFlow::Vertical, ReadingFlow::Horizontal] {
            for animation_enabled in [true, false] {
                let case = format!("{mode:?}/{flow:?} animation={animation_enabled}");
                eprintln!("round-trip: {case}");
                let ctx = egui::Context::default();
                install_fs_navigator_input_tracking(&ctx);
                let mut app = book(&ctx, true, ReadingFlow::Paged, mode, animation_enabled);
                let probe = Probe::start();
                assert!(app.fs_pdf_display_target.is_none(), "{case}");
                let mut history = Vec::new();
                for _ in 0..12 {
                    draw_paged(&mut app, &ctx);
                    let requests = probe.take_requests();
                    history.extend(requests.iter().cloned());
                    complete_request_batch(&mut app, &ctx, &probe, true, requests);
                }
                let paged_target = app.fs_pdf_display_target.expect(&case);
                let paged_pages = app
                    .fullscreen_page_layout
                    .page_indices()
                    .collect::<HashSet<_>>();
                assert_eq!(
                    paged_pages.len(),
                    if mode.is_spread() { 2 } else { 1 },
                    "paged fixture: {case}"
                );
                for &idx in &paged_pages {
                    assert!(
                        app.resolve_fs_display_material(idx, false).is_some(),
                        "paged must be materialized before switching: idx={idx} {case}"
                    );
                }
                let viewport = app.fullscreen_viewport_id();
                let generation = app.items_generation;
                let idx = app.fullscreen_idx.unwrap();
                // This is the setter used by FsReadingFlowCycle (key 6) and the HUD.
                app.set_reading_flow_for_fullscreen(&ctx, idx, flow);
                assert_eq!(app.reading_flow, flow, "switch rejected: {case}");
                assert_eq!(
                    app.fullscreen_idx,
                    Some(idx),
                    "switch reopened/navigated: {case}"
                );
                // Require more pixels than the paged raster already provides.
                // Center the viewport between units below so both remain visible
                // even when one continuous page is larger than the viewport.
                app.fs_zoom = 1.2;
                let expected_target = crate::pdf_loader::PdfDisplayTarget::from_logical_size(
                    2400.0,
                    1600.0,
                    ctx.pixels_per_point(),
                    if flow == ReadingFlow::Vertical {
                        crate::pdf_loader::PdfDisplayFitMode::Width
                    } else {
                        crate::pdf_loader::PdfDisplayFitMode::Height
                    },
                );
                assert_ne!(
                    expected_target, paged_target,
                    "fixture must distinguish stale paged target: {case}"
                );
                let history_boundary = history.len();
                let mut sources = draw(&mut app, &ctx);
                let current = app.fullscreen_idx.unwrap();
                let (units, pos) = app.continuous_reading_units_and_pos(current).expect(&case);
                assert!(
                    pos + 1 < units.len(),
                    "fixture needs an adjacent unit: {case}"
                );
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2400.0, 1600.0));
                let (_, _, offsets, _) = app
                    .continuous_reading_layout(&ctx, rect, &units, pos, false)
                    .expect(&case);
                app.fs_vertical_scroll = (offsets[pos] + offsets[pos + 1]) * 0.5;
                for _ in 0..12 {
                    assert_eq!(
                        app.fs_pdf_display_target,
                        Some(expected_target),
                        "stale paged PDF target in continuous: {case}"
                    );
                    let requests = probe.take_requests();
                    for request in &requests {
                        if request.target_px.is_none() {
                            assert_eq!(
                                request.pdf_target,
                                Some(expected_target),
                                "request kept paged geometry: idx={} {case}",
                                request.idx
                            );
                        }
                    }
                    history.extend(requests.iter().cloned());
                    complete_request_batch(&mut app, &ctx, &probe, true, requests);
                    sources = draw(&mut app, &ctx);
                }
                assert!(
                    sources.pages.len() >= 2,
                    "continuous fixture must show neighbors: {case}"
                );
                assert!(
                    sources
                        .pages
                        .iter()
                        .any(|page| !paged_pages.contains(&page.occurrence.idx)),
                    "fixture must expose a previously unseen page: {case}"
                );
                for page in sources.pages {
                    let idx = page.occurrence.idx;
                    let layout = app.fullscreen_page_layout.page_by_idx(idx).expect(&case);
                    let rect = layout.transform.layout_full_image_rect;
                    let required = rect.width().max(rect.height()) * ctx.pixels_per_point();
                    let size = page.resource.source_texture().expect(&case).size();
                    assert!(
                        size[0].max(size[1]) as f32 >= required && size[1] > 561,
                        "visible page is undersampled: idx={idx} raster={size:?} required={required} {case}"
                    );
                    assert!(
                        history.iter().any(|request| request.idx == idx
                            && pdf_request_long_edge(request) as f32 >= required),
                        "no request covers continuous drawing dimensions: idx={idx} required={required} {case}"
                    );
                    let paged_long = history[..history_boundary]
                        .iter()
                        .filter(|request| request.idx == idx)
                        .map(pdf_request_long_edge)
                        .max()
                        .unwrap_or(0);
                    assert!(
                        (paged_long as f32) < required,
                        "fixture must need new continuous resolution: idx={idx} paged={paged_long} required={required} {case}"
                    );
                    assert!(
                        history[history_boundary..]
                            .iter()
                            .any(|request| request.idx == idx
                                && pdf_request_long_edge(request) as f32 >= required),
                        "visible page received no sufficient continuous request: idx={idx} {case}"
                    );
                }
                assert_eq!(app.fullscreen_viewport_id(), viewport, "{case}");
                assert_eq!(app.items_generation, generation, "{case}");

                let idx = app.fullscreen_idx.unwrap();
                app.set_reading_flow_for_fullscreen(&ctx, idx, ReadingFlow::Paged);
                assert_eq!(
                    app.reading_flow,
                    ReadingFlow::Paged,
                    "reverse switch rejected: {case}"
                );
                assert_eq!(
                    app.fullscreen_idx,
                    Some(idx),
                    "reverse switch navigated: {case}"
                );
                for _ in 0..12 {
                    draw_paged(&mut app, &ctx);
                    let requests = probe.take_requests();
                    history.extend(requests.iter().cloned());
                    complete_request_batch(&mut app, &ctx, &probe, true, requests);
                }
                assert_eq!(
                    app.fs_pdf_display_target,
                    Some(paged_target),
                    "reverse switch kept continuous target: {case}"
                );
                assert!(
                    app.fullscreen_page_layout
                        .page_indices()
                        .any(|page| page == idx),
                    "paged lost current page: {case}"
                );
                let rect = app
                    .fullscreen_page_layout
                    .page_by_idx(idx)
                    .expect(&case)
                    .transform
                    .layout_full_image_rect;
                let required = rect.width().max(rect.height()) * ctx.pixels_per_point();
                let resource = app.resolve_fs_display_material(idx, false).expect(&case);
                let size = resource.source_texture().size();
                assert!(
                    size[0].max(size[1]) as f32 >= required && size[1] > 561,
                    "reverse paged current is undersampled: idx={idx} {case}"
                );
                assert!(
                    history.iter().any(|request| request.idx == idx
                        && pdf_request_long_edge(request) as f32 >= required),
                    "reverse paged current has no sufficient request: idx={idx} {case}"
                );
            }
        }
    }
}

#[test]
fn section1674_continuous_refine_rejoins_after_cancel_and_failure_is_terminal() {
    let ctx = egui::Context::default();
    let mut app = book(
        &ctx,
        true,
        ReadingFlow::Horizontal,
        SpreadMode::Single,
        true,
    );
    let probe = Probe::start();
    for _ in 0..16 {
        draw(&mut app, &ctx);
        complete_requests(&mut app, &ctx, &probe, true);
    }
    app.fs_zoom = 2.0;
    draw(&mut app, &ctx);
    let request = probe
        .take_requests()
        .into_iter()
        .find(|request| request.target_px.is_some())
        .expect("zoomed visible page must refine");
    let idx = request.idx;
    assert_eq!(
        request.priority,
        crate::fs_page_load_scheduler::FsPageLoadPriority::High
    );
    let old_cancel = app.fs_pending.get(&idx).unwrap().cancel.clone();
    app.cancel_fs_page_load(idx);
    assert!(old_cancel.load(std::sync::atomic::Ordering::Relaxed));
    draw(&mut app, &ctx);
    let resumed = probe.take_requests();
    assert!(
        resumed
            .iter()
            .any(|next| next.idx == idx && next.target_px == request.target_px),
        "same geometry must re-enter after its pending was withdrawn"
    );
    probe.complete(idx, FsLoadResult::Failed);
    app.poll_prefetch(&ctx, PollPrefetchOrigin::TopLevel);
    for _ in 0..4 {
        draw(&mut app, &ctx);
    }
    assert!(
        !probe
            .take_requests()
            .iter()
            .any(|request| request.idx == idx),
        "a failed refinement is not an automatic retry"
    );
    assert!(
        matches!(app.fs_cache.get(&idx), Some(FsCacheEntry::Static { .. })),
        "failed refine must preserve the displayed raster"
    );
}

#[test]
fn section1674_completed_continuous_pdf_stops_requesting_16ms_repaint() {
    let ctx = egui::Context::default();
    let mut app = book(&ctx, true, ReadingFlow::Horizontal, SpreadMode::Rtl, true);
    let probe = Probe::start();
    for _ in 0..24 {
        draw(&mut app, &ctx);
        complete_requests(&mut app, &ctx, &probe, true);
    }
    for _ in 0..3 {
        let (_, delay) = draw_with_delay(&mut app, &ctx);
        assert!(
            delay > std::time::Duration::from_millis(16),
            "materialization must settle without a repaint workaround"
        );
        assert!(probe.take_requests().is_empty());
    }
}

#[test]
fn section1674_hidden_pages_do_not_receive_visible_quality_demand() {
    let ctx = egui::Context::default();
    let mut app = book(
        &ctx,
        true,
        ReadingFlow::Horizontal,
        SpreadMode::Single,
        true,
    );
    let (units, _) = app.continuous_reading_units_and_pos(3).unwrap();
    let hidden = units
        .iter()
        .filter(|unit| unit.contains_idx(4))
        .flat_map(ContinuousReadingUnitSpec::presentation_demand)
        .collect::<Vec<_>>();
    app.install_hidden_presentation_for_test(&hidden);
    let probe = Probe::start();
    let sources = draw(&mut app, &ctx);
    assert!(!sources.pages.iter().any(|page| page.occurrence.idx == 4));
    assert!(!probe.take_requests().iter().any(|request| request.idx == 4
        && request.priority == crate::fs_page_load_scheduler::FsPageLoadPriority::High));
}

#[test]
fn section1674_visible_neighbor_keeps_raw_load_but_retires_old_animation_promotion() {
    let ctx = egui::Context::default();
    let mut app = book(
        &ctx,
        false,
        ReadingFlow::Horizontal,
        SpreadMode::Single,
        true,
    );
    let probe = Probe::start();
    let old_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (_sender, receiver) = std::sync::mpsc::channel();
    app.fs_pending.insert(
        4,
        crate::app::FsPendingValue::new(
            old_cancel.clone(),
            receiver,
            0,
            crate::app::FsLoadPurpose::AnimationPromotion {
                target_idx: 4,
                started_at: std::time::Instant::now(),
            },
        ),
    );
    let sources = draw(&mut app, &ctx);
    assert!(sources.pages.iter().any(|page| page.occurrence.idx == 4));
    assert!(old_cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert!(probe.take_requests().iter().any(|request| request.idx == 4));
    assert_eq!(
        app.fs_pending.get(&4).unwrap().purpose,
        crate::app::FsLoadPurpose::Prefetch
    );
}
