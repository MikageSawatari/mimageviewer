//! Directional arrivals use the ordinary navigation and material-selection paths.
use super::handler_tests::{app as scroll_app, frame, full_rect, paint};
use super::*;
use crate::settings::{FullscreenNScrollOrder, FullscreenPageStartPosition};

pub(super) fn install_material(app: &mut App, ctx: &egui::Context, idx: usize, size: [usize; 2]) {
    let pixels = Arc::new(egui::ColorImage::filled(size, egui::Color32::WHITE));
    let tex = ctx.load_texture(
        format!("landing-{idx}-{}-{}", size[0], size[1]),
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
            source_dims: Some(size),
            load_seq: 2,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
    app.record_page_dims_for_spread(idx, (size[0] as u32, size[1] as u32));
}

fn material_app(
    ctx: &egui::Context,
    mode: SpreadMode,
    size: [usize; 2],
) -> crate::app::AppTestEnvForTest {
    let mut app = scroll_app(ctx, mode);
    app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::Center;
    app.settings.fullscreen_fit_mode = FullscreenFitMode::Original;
    for idx in 0..app.items.len() {
        install_material(&mut app, ctx, idx, size);
    }
    app.reconcile_fullscreen_page_slice(0);
    app
}

fn navigate(app: &mut App, ctx: &egui::Context, nav: FsPageNav, landing: Option<FsPageLanding>) {
    let current = app.fullscreen_idx.unwrap();
    app.handle_fs_navigation(
        ctx,
        false,
        false,
        None,
        None,
        None,
        FsPageNavRequest { nav, landing },
        None,
        current,
    );
}

fn painted_unit(app: &App) -> Rect {
    app.fullscreen_page_layout
        .displayed_pages()
        .map(|page| page.transform.paint_rect)
        .reduce(Rect::union)
        .expect("complete current display unit")
}

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 1.01,
        "actual={actual}, expected={expected}"
    );
}

fn assert_corner(app: &App, landing: FsPageLanding, direction: ReadingDirection) {
    let unit = painted_unit(app);
    let viewport = full_rect();
    if unit.height() > viewport.height() {
        match landing {
            FsPageLanding::Start => close(unit.top(), viewport.top()),
            FsPageLanding::End => close(unit.bottom(), viewport.bottom()),
        }
    }
    if unit.width() > viewport.width() {
        match (landing, direction) {
            (FsPageLanding::Start, ReadingDirection::Ltr)
            | (FsPageLanding::End, ReadingDirection::Rtl) => close(unit.left(), viewport.left()),
            _ => close(unit.right(), viewport.right()),
        }
    }
}

#[test]
fn manual_navigation_lands_at_the_reading_corner_for_every_unit_and_order() {
    for mode in [
        SpreadMode::Single,
        SpreadMode::Ltr,
        SpreadMode::Rtl,
        SpreadMode::SplitLtr,
        SpreadMode::SplitRtl,
    ] {
        for book_direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
            for order in [
                FullscreenNScrollOrder::VerticalFirst,
                FullscreenNScrollOrder::HorizontalFirst,
            ] {
                for landing in [FsPageLanding::Start, FsPageLanding::End] {
                    for overflow in [0, 1, 2] {
                        let ctx = egui::Context::default();
                        let width = if overflow == 0 { 240 } else { 900 };
                        let height = if overflow == 1 { 200 } else { 800 };
                        let mut app = material_app(
                            &ctx,
                            mode,
                            [width * if mode.is_split() { 2 } else { 1 }, height],
                        );
                        app.reading_direction = book_direction;
                        app.settings.fullscreen_n_scroll_order = order;
                        let nav = if mode.is_split() {
                            FsPageNav::Split(crate::page_split::PresentationStep {
                                source_idx: 2,
                                slice: crate::page_split::SplitDirection::from_spread_mode(mode)
                                    .unwrap()
                                    .second(),
                            })
                        } else {
                            FsPageNav::Target(2)
                        };
                        navigate(&mut app, &ctx, nav, Some(landing));
                        assert_eq!(app.fullscreen_idx, Some(2));
                        assert_eq!(
                            app.fs_initial_placement_request.unwrap().kind,
                            landing.initial_placement_kind()
                        );
                        frame(&ctx, vec![], |ui, ctx| {
                            paint(&mut app, ui, ctx);
                        });
                        let unit = painted_unit(&app);
                        assert_eq!(
                            unit.width() > full_rect().width(),
                            overflow != 0,
                            "{mode:?} {overflow}"
                        );
                        assert_eq!(
                            unit.height() > full_rect().height(),
                            overflow != 1,
                            "{mode:?} {overflow}"
                        );
                        assert_corner(
                            &app,
                            landing,
                            mode.reading_direction().unwrap_or(book_direction),
                        );
                        assert_eq!(app.fs_zoom, 1.0);
                        assert!(app.fs_initial_placement_request.is_none());
                        assert!(!app.fs_view_has_moved(
                            app.fs_zoom,
                            app.fs_pan,
                            app.fs_free_rotation
                        ));
                    }
                }
            }
        }
    }
}

#[test]
fn same_image_split_landing_preserves_zoom_and_the_fitting_axis() {
    for mode in [SpreadMode::SplitLtr, SpreadMode::SplitRtl] {
        for horizontal in [false, true] {
            for landing in [FsPageLanding::Start, FsPageLanding::End] {
                let ctx = egui::Context::default();
                let mut app = material_app(
                    &ctx,
                    mode,
                    if horizontal { [1800, 200] } else { [480, 800] },
                );
                app.fs_zoom = 1.25;
                app.fs_pan = Vec2::new(17.0, 23.0);
                frame(&ctx, vec![], |ui, ctx| {
                    paint(&mut app, ui, ctx);
                });
                let origin = app.fs_pan;
                let direction = crate::page_split::SplitDirection::from_spread_mode(mode).unwrap();
                navigate(
                    &mut app,
                    &ctx,
                    FsPageNav::Split(crate::page_split::PresentationStep {
                        source_idx: 0,
                        slice: direction.second(),
                    }),
                    Some(landing),
                );
                assert_eq!(app.fullscreen_page_slice, direction.second());
                assert_eq!((app.fs_zoom, app.fs_pan), (1.25, origin));
                frame(&ctx, vec![], |ui, ctx| {
                    paint(&mut app, ui, ctx);
                });
                assert_eq!(app.fs_zoom, 1.25);
                assert_eq!(
                    app.fs_pan[if horizontal { 1 } else { 0 }],
                    origin[if horizontal { 1 } else { 0 }]
                );
                assert_corner(&app, landing, mode.reading_direction().unwrap());
                assert!(app.fs_initial_placement_request.is_none());
            }
        }
    }
}

#[test]
fn landing_kind_survives_provisional_material_and_center_setting_until_full_arrival() {
    use crate::ui_fullscreen::initial_placement::tests::{
        install_arriving_full, paint_prepared_single, thumbnail_arrival_app,
    };
    for landing in [FsPageLanding::Start, FsPageLanding::End] {
        for direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
            let ctx = egui::Context::default();
            let mut app = thumbnail_arrival_app(&ctx, FullscreenFitMode::Original);
            app.reading_direction = direction;
            app.request_fs_initial_placement_with_kind(0, landing.initial_placement_kind());
            frame(&ctx, vec![], |ui, ctx| {
                paint_prepared_single(&mut app, ui, ctx);
                assert_eq!(
                    app.fs_initial_placement_request.unwrap().kind,
                    landing.initial_placement_kind()
                );
                assert_corner(&app, landing, direction);
            });
            let provisional = app.fs_pan;
            app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::Center;
            app.retire_fs_initial_placement_request();
            assert_eq!(
                app.fs_initial_placement_request.unwrap().kind,
                landing.initial_placement_kind()
            );
            install_arriving_full(&mut app, &ctx, 0);
            frame(&ctx, vec![], |ui, ctx| {
                paint_prepared_single(&mut app, ui, ctx);
                assert_corner(&app, landing, direction);
                assert!(app.fs_initial_placement_request.is_none());
                assert!(!app.fs_view_has_moved(app.fs_zoom, app.fs_pan, app.fs_free_rotation));
                // Full source geometry differs from its provisional thumbnail.
                assert_ne!(app.fs_pan, provisional);
            });
        }
    }
}

#[test]
fn changing_global_start_position_retires_only_setting_requests() {
    use crate::ui_fullscreen::initial_placement::{
        FsInitialPlacementKind, tests::thumbnail_arrival_app,
    };
    for kind in [
        FsInitialPlacementKind::Setting,
        FsInitialPlacementKind::LandingStart,
        FsInitialPlacementKind::LandingEnd,
    ] {
        let ctx = egui::Context::default();
        let mut app = thumbnail_arrival_app(&ctx, FullscreenFitMode::Original);
        app.request_fs_initial_placement_with_kind(0, kind);
        app.settings.fullscreen_page_start_position = FullscreenPageStartPosition::Center;
        app.retire_fs_initial_placement_request();
        assert_eq!(
            app.fs_initial_placement_request.map(|request| request.kind),
            if kind == FsInitialPlacementKind::Setting {
                None
            } else {
                Some(kind)
            }
        );
    }
}

#[test]
fn landing_end_survives_composition_reanchor_and_is_consumed_on_the_new_unit() {
    use crate::ui_fullscreen::initial_placement::{
        FsInitialPlacementKind,
        tests::{composition_switch_app, paint_composition_switch_unit},
    };
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        let ctx = egui::Context::default();
        let mut app =
            composition_switch_app(&ctx, SpreadMode::Single, false, ReadingDirection::Ltr);
        app.fullscreen_idx = Some(1);
        app.fs_zoom = 1.5;
        app.request_fs_initial_placement_with_kind(1, FsInitialPlacementKind::LandingEnd);
        assert!(app.apply_fullscreen_spread_mode(&ctx, 1, mode));
        assert_eq!(
            app.fullscreen_idx,
            Some(0),
            "ordinary normalization reopens the pair anchor"
        );
        let request = app
            .fs_initial_placement_request
            .expect("reanchor retains operation kind");
        assert_eq!(request.kind, FsInitialPlacementKind::LandingEnd);
        assert_eq!(request.anchor_idx, 0);
        assert_eq!(
            request.origin_zoom, 1.0,
            "ordinary reopen keeps its existing zoom rule"
        );
        frame(&ctx, vec![], |ui, ctx| {
            paint_composition_switch_unit(&mut app, ui, ctx);
            assert_corner(&app, FsPageLanding::End, mode.reading_direction().unwrap());
            assert!(app.fs_initial_placement_request.is_none());
        });
    }
}

#[test]
fn manual_pan_retires_provisional_landing_before_full_material_arrives() {
    use crate::ui_fullscreen::initial_placement::tests::{
        install_arriving_full, paint_prepared_single, thumbnail_arrival_app,
    };
    for landing in [FsPageLanding::Start, FsPageLanding::End] {
        let ctx = egui::Context::default();
        let mut app = thumbnail_arrival_app(&ctx, FullscreenFitMode::Original);
        app.request_fs_initial_placement_with_kind(0, landing.initial_placement_kind());
        frame(&ctx, vec![], |ui, ctx| {
            paint_prepared_single(&mut app, ui, ctx);
        });
        assert!(app.fs_initial_placement_request.is_some());
        let zoom = app.fs_zoom;
        let requested = app.fs_pan
            + egui::vec2(
                0.0,
                if landing == FsPageLanding::Start {
                    -12.0
                } else {
                    12.0
                },
            );
        app.set_fs_pan_from_input(zoom, requested);
        let manual = app.fs_pan;
        assert!(app.fs_initial_placement_request.is_none());
        install_arriving_full(&mut app, &ctx, 0);
        frame(&ctx, vec![], |ui, ctx| {
            paint_prepared_single(&mut app, ui, ctx);
        });
        assert_eq!(app.fs_pan, manual);
        assert!(app.fs_initial_placement_request.is_none());
    }
}

#[test]
fn scroll_turn_boundaries_and_wrap_use_the_existing_units_and_directional_arrival() {
    for mode in [
        SpreadMode::Single,
        SpreadMode::Ltr,
        SpreadMode::Rtl,
        SpreadMode::SplitLtr,
        SpreadMode::SplitRtl,
    ] {
        for next in [false, true] {
            for wrap in [false, true] {
                let ctx = egui::Context::default();
                let mut app = material_app(
                    &ctx,
                    mode,
                    if mode.is_split() {
                        [1800, 800]
                    } else {
                        [900, 800]
                    },
                );
                app.settings.fullscreen_manual_nav_wrap = wrap;
                let nav = app.get_nav_indices();
                let (first, last, first_slice, last_slice) = if mode.is_split() {
                    let steps = app.build_presentation_steps_for_nav(&nav).unwrap();
                    let first = steps[0];
                    let last = *steps.last().unwrap();
                    (first.source_idx, last.source_idx, first.slice, last.slice)
                } else if mode.is_spread() {
                    let units = app.build_spread_display_units_for_nav(&nav);
                    (
                        units.first().unwrap().anchor_idx(),
                        units.last().unwrap().anchor_idx(),
                        crate::page_split::PageSlice::Full,
                        crate::page_split::PageSlice::Full,
                    )
                } else {
                    (
                        nav[0],
                        *nav.last().unwrap(),
                        crate::page_split::PageSlice::Full,
                        crate::page_split::PageSlice::Full,
                    )
                };
                app.fullscreen_idx = Some(if next { last } else { first });
                app.fullscreen_page_slice = if next { last_slice } else { first_slice };
                super::handler_tests::seed(&ctx, &mut app);
                let original = app.fullscreen_idx;
                frame(&ctx, vec![], |_, ctx| {
                    for _ in 0..200 {
                        let before = app.fs_pan;
                        app.apply_fs_page_scroll_key(
                            ctx,
                            if next {
                                KeyAction::FsPageNScrollNext
                            } else {
                                KeyAction::FsPageNScrollPrev
                            },
                        );
                        if app.fs_pan == before {
                            break;
                        }
                    }
                    let expected = app.spread_page_nav(if next { 1 } else { -1 });
                    let request = app.fs_page_scroll_or_turn_key(
                        ctx,
                        next,
                        crate::keymap::PageTurnPressKind::InitialPress,
                    );
                    assert_eq!(request.nav, expected);
                    assert_eq!(
                        request.landing,
                        Some(if next {
                            FsPageLanding::Start
                        } else {
                            FsPageLanding::End
                        })
                    );
                    if wrap {
                        assert!(matches!(request.nav, FsPageNav::Wrap(_)));
                    } else {
                        assert!(matches!(request.nav,FsPageNav::Boundary {at_end} if at_end==next));
                    }
                    let idx = app.fullscreen_idx.unwrap();
                    app.handle_fs_navigation(
                        ctx, false, false, None, None, None, request, None, idx,
                    );
                });
                if wrap {
                    assert_eq!(app.fullscreen_idx, Some(if next { first } else { last }));
                    assert_eq!(
                        app.fullscreen_page_slice,
                        if next { first_slice } else { last_slice }
                    );
                    frame(&ctx, vec![], |ui, ctx| {
                        paint(&mut app, ui, ctx);
                    });
                    assert_corner(
                        &app,
                        if next {
                            FsPageLanding::Start
                        } else {
                            FsPageLanding::End
                        },
                        mode.reading_direction().unwrap_or(app.reading_direction),
                    );
                } else {
                    assert_eq!(app.fullscreen_idx, original);
                    assert!(app.fs_initial_placement_request.is_none());
                }
            }
        }
    }
}
