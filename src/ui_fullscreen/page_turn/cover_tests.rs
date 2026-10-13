//! Cover and tail diagnostics through the ordinary unit builder and renderer.
#![cfg(test)]
use super::*;
use crate::ui_fullscreen::page_turn_submission_tests::{draw_current, frame, ready_app};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundaryDiagnosis {
    OutgoingCanonical(&'static str),
    IncomingCanonical(&'static str),
    TargetAdmission(&'static str),
    OutgoingAdmission(&'static str),
    Ready,
}

fn book(
    ctx: &egui::Context,
    mode: SpreadMode,
    pages: usize,
    first_side: bool,
    last_side: bool,
    supplement: bool,
) -> crate::app::AppTestEnvForTest {
    let mut app = ready_app(ctx, mode);
    let folder = PathBuf::from("c:/page-turn-cover-book");
    app.items = (0..pages)
        .map(|idx| GridItem::Image(folder.join(format!("{:02}.png", idx + 1))))
        .collect();
    app.current_folder = Some(folder);
    app.top_level_grid_view
        .replace_surface(crate::app::top_level_grid_view::TopLevelGridSurface::Folder);
    app.visible_indices = (0..pages).collect();
    app.details_order = (0..pages).collect();
    app.items_generation += 1;
    app.fs_cache.clear();
    app.thumbnails = (0..pages).map(|_| ThumbnailState::Pending).collect();
    app.settings.page_turn_animation_enabled = true;
    app.settings.page_turn_animation_duration_ms = 600;
    app.settings.singleton_spread_first_enabled = first_side;
    app.settings.singleton_spread_last_enabled = last_side;
    app.settings.final_cover_spread_enabled = supplement;
    app.final_cover_spread_preference = crate::settings::FinalCoverSpreadPreference::FollowGlobal;
    app.singleton_spread_endpoint_preferences = Default::default();
    app.settings.page_after_cover_alone_enabled = false;
    app.settings.last_page_alone_enabled = false;
    app.page_alone_preferences = Default::default();
    for idx in 0..pages {
        // The reported 1200x1700 / landscape book, uniformly reduced by ten.
        let dims = if [3, 6, 7].contains(&idx) {
            [170, 120]
        } else {
            [120, 170]
        };
        let image =
            egui::ColorImage::filled(dims, egui::Color32::from_rgb(40 + idx as u8 * 12, 100, 160));
        let texture = ctx.load_texture(
            format!("cover-book-{idx}"),
            image.clone(),
            egui::TextureOptions::LINEAR,
        );
        app.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex: texture,
                pixels: Arc::new(image),
                source_dims: Some(dims),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        app.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
        app.record_page_dims_for_spread(idx, (dims[0] as u32, dims[1] as u32));
    }
    app.reconcile_spread_landscapes_from_rotation_cache();
    install(ctx);
    app
}

fn draw_observed(
    app: &mut App,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
) -> FsNavigatorTextureSources {
    let (composition, sources) = draw_current(app, ui, ctx);
    let decision = app.fs_page_turn_decision_for_frame(ctx, app.fullscreen_idx.unwrap());
    app.emit_fs_page_turn_ready_for_display_unit(ctx, &composition, decision, &sources);
    sources
}

fn capture(ctx: &egui::Context, app: &mut App, anchor: usize) -> Arc<PaintedUnit> {
    app.fullscreen_idx = Some(anchor);
    frame(ctx, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        draw_observed(app, ui, ctx);
    });
    let PresentationState::Flat(unit) = &app.page_turn_presentation.state else {
        panic!("ordinary prepared flat for anchor {anchor}");
    };
    assert!(unit.evidence.was_accepted());
    unit.clone()
}

fn cover_side(mode: SpreadMode) -> SingletonSpreadPlacement {
    if mode.is_rtl() {
        SingletonSpreadPlacement::Left
    } else {
        SingletonSpreadPlacement::Right
    }
}

fn actual_side(transform: &DisplayedImageTransform) -> SingletonSpreadPlacement {
    match transform.placement {
        ResolvedDisplayPlacement::SingletonSpread { side, .. } => side,
        ResolvedDisplayPlacement::Z { singleton_side, .. } => singleton_side,
        ResolvedDisplayPlacement::Normal { .. } => SingletonSpreadPlacement::Center,
    }
}

fn adopt_off_then_optional_on(app: &mut App, final_enabled: bool) {
    let mut settings = app.settings.clone();
    settings.page_turn_animation_enabled = false;
    app.install_preferences_settings(settings);
    if final_enabled {
        let mut settings = app.settings.clone();
        settings.page_turn_animation_enabled = true;
        app.install_preferences_settings(settings);
    }
}

fn diagnosis(outgoing: &PaintedUnit, incoming: &PaintedUnit) -> BoundaryDiagnosis {
    for (unit, target) in [(incoming, true), (outgoing, false)] {
        if let Some(reason) = stage_one_shape_reason(
            unit.mode,
            unit.pages.len(),
            unit.pages
                .iter()
                .any(|p| p.occurrence.role != SpreadPageRole::Navigation),
            unit.white.is_some(),
        ) {
            return if target {
                BoundaryDiagnosis::TargetAdmission(reason)
            } else {
                BoundaryDiagnosis::OutgoingAdmission(reason)
            };
        }
    }
    if let Err(reason) = canonical_unit(outgoing) {
        return BoundaryDiagnosis::OutgoingCanonical(reason);
    }
    if let Err(reason) = canonical_unit(incoming) {
        return BoundaryDiagnosis::IncomingCanonical(reason);
    }
    BoundaryDiagnosis::Ready
}

fn navigate(
    ctx: &egui::Context,
    app: &mut App,
    from: usize,
    to: usize,
    forward: bool,
    expected: BoundaryDiagnosis,
) {
    let incoming = capture(ctx, app, to);
    let outgoing = capture(ctx, app, from);
    assert_eq!(diagnosis(&outgoing, &incoming), expected, "{from}->{to}");
    take_test_events();
    let operation = ViewerOperationId::Slideshow(next_operation_batch());
    frame(ctx, |ui, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
        app.page_turn_external_operation(ctx, operation, OperationPhase::Press, "cover-regression");
        app.handle_fs_navigation_with_operation(
            ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNav::Target(to),
            None,
            from,
            Some(operation),
        );
        assert_eq!(app.fullscreen_idx, Some(to));
        let admission_rejected = matches!(
            expected,
            BoundaryDiagnosis::TargetAdmission(_) | BoundaryDiagnosis::OutgoingAdmission(_)
        );
        assert_eq!(
            matches!(
                app.page_turn_presentation.state,
                PresentationState::Offered { .. }
            ),
            !admission_rejected,
            "the actual owner must reach the diagnosed phase {expected:?}",
        );
        let sources = draw_observed(app, ui, ctx);
        let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
            panic!("ordinary paged submission");
        };
        assert_eq!(receipt.curl, expected == BoundaryDiagnosis::Ready);
    });
    // Started is adopted by the final-output hook, after ctx.run returns.
    let tracker = if expected == BoundaryDiagnosis::Ready {
        let PresentationState::Turning { offer, .. } = &mut app.page_turn_presentation.state else {
            panic!("started turn {from}->{to}");
        };
        assert_eq!(offer.forward, forward);
        assert_eq!(offer.submission.status(), SubmissionStatus::Started);
        offer.accepted_at -= offer.duration + Duration::from_millis(1);
        Some(offer.submission.clone())
    } else {
        None
    };
    if let Some(tracker) = tracker {
        frame(ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            let sources = draw_observed(app, ui, ctx);
            let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                panic!("canonical finish");
            };
            assert!(!receipt.curl);
        });
        assert_eq!(tracker.status(), SubmissionStatus::Finished);
    }
    let events: Vec<_> = take_test_events()
        .into_iter()
        .filter(|(fact, _, _)| fact.operation_id == operation)
        .collect();
    match expected {
        BoundaryDiagnosis::Ready => {
            for kind in ["opportunity", "offered", "start", "finish", "fully_visible"] {
                assert_eq!(
                    events.iter().filter(|(_, k, _)| *k == kind).count(),
                    1,
                    "{events:?}"
                );
            }
            assert!(
                !events
                    .iter()
                    .any(|(_, k, _)| matches!(*k, "skip" | "cancel"))
            );
        }
        BoundaryDiagnosis::OutgoingCanonical(_) | BoundaryDiagnosis::IncomingCanonical(_) => {
            assert_eq!(events.iter().filter(|(_, k, _)| *k == "offered").count(), 1);
            assert_eq!(
                events
                    .iter()
                    .filter(|(_, k, r)| *k == "skip" && *r == "geometry_visibility_or_roi")
                    .count(),
                1
            );
            assert!(
                !events
                    .iter()
                    .any(|(_, k, _)| matches!(*k, "start" | "cancel"))
            );
        }
        BoundaryDiagnosis::TargetAdmission(reason)
        | BoundaryDiagnosis::OutgoingAdmission(reason) => {
            assert_eq!(
                events
                    .iter()
                    .filter(|(_, k, r)| *k == "skip" && *r == reason)
                    .count(),
                1
            );
            assert!(
                !events
                    .iter()
                    .any(|(_, k, _)| matches!(*k, "offered" | "start" | "cancel"))
            );
        }
    }
    frame(ctx, |_, ctx| {
        app.begin_page_turn_viewer_frame(ctx, false, None);
    });
}

#[test]
fn page_turn_reported_cover_first_pair_eight_routes_start_and_complete() {
    let mut routes = 0;
    for mode in [SpreadMode::LtrCover, SpreadMode::RtlCover] {
        for side in [false, true] {
            for forward in [true, false] {
                let ctx = egui::Context::default();
                let mut app = book(&ctx, mode, 12, side, side, false);
                let units = app.build_spread_display_units_for_nav(&(0..12).collect::<Vec<_>>());
                assert_eq!(
                    units.iter().map(|u| u.anchor_idx()).collect::<Vec<_>>(),
                    vec![0, 1, 3, 4, 6, 7, 8, 10]
                );
                assert_eq!(units[0].pages, vec![0]);
                assert_eq!(units[1].pages, vec![1, 2]);
                assert_eq!(app.settings.singleton_spread_first_enabled, side);
                assert_eq!(
                    app.spread_display_composition_for_anchor(0)
                        .singleton_placement(),
                    cover_side(mode)
                );
                navigate(
                    &ctx,
                    &mut app,
                    if forward { 0 } else { 1 },
                    if forward { 1 } else { 0 },
                    forward,
                    BoundaryDiagnosis::Ready,
                );
                routes += 1;
            }
        }
    }
    assert_eq!(routes, 8);
}

#[test]
fn page_turn_reported_twelve_page_tail_remains_pair_with_or_without_supplement() {
    for mode in [SpreadMode::LtrCover, SpreadMode::RtlCover] {
        for side in [false, true] {
            for supplement in [false, true] {
                let ctx = egui::Context::default();
                let mut app = book(&ctx, mode, 12, side, side, supplement);
                let units = app.build_spread_display_units_for_nav(&(0..12).collect::<Vec<_>>());
                assert_eq!(units.last().unwrap().pages, vec![10, 11]);
                let tail =
                    app.spread_display_composition_for_anchor(units.last().unwrap().anchor_idx());
                assert_eq!(tail.singleton_placement(), SingletonSpreadPlacement::Center);
                assert!(
                    tail.pages_in_reading_order()
                        .iter()
                        .all(|p| p.role == SpreadPageRole::Navigation)
                );
                for (from, to, forward) in [(8, 10, true), (10, 8, false)] {
                    navigate(&ctx, &mut app, from, to, forward, BoundaryDiagnosis::Ready);
                }
            }
        }
    }
}

#[test]
fn page_turn_tail_singleton_supplement_diagnoses_admission_without_changing_builder() {
    let mut routes = 0;
    for mode in [SpreadMode::LtrCover, SpreadMode::RtlCover] {
        for side in [false, true] {
            for supplement in [false, true] {
                for forward in [true, false] {
                    let ctx = egui::Context::default();
                    let mut app = book(&ctx, mode, 11, side, side, supplement);
                    let units =
                        app.build_spread_display_units_for_nav(&(0..11).collect::<Vec<_>>());
                    assert_eq!(units.last().unwrap().pages, vec![10]);
                    let tail = app.spread_display_composition_for_anchor(10);
                    assert_eq!(
                        tail.pages_in_reading_order().len(),
                        if supplement { 2 } else { 1 }
                    );
                    assert_eq!(
                        tail.pages_in_reading_order()
                            .iter()
                            .any(|p| p.role == SpreadPageRole::FrontCoverSupplement),
                        supplement
                    );
                    let expected = if !supplement {
                        BoundaryDiagnosis::Ready
                    } else if forward {
                        BoundaryDiagnosis::TargetAdmission("stage1_cover_or_single_companion")
                    } else {
                        BoundaryDiagnosis::OutgoingAdmission("stage1_cover_or_single_companion")
                    };
                    navigate(
                        &ctx,
                        &mut app,
                        if forward { 8 } else { 10 },
                        if forward { 10 } else { 8 },
                        forward,
                        expected,
                    );
                    routes += 1;
                }
            }
        }
    }
    assert_eq!(routes, 16);
}

#[test]
fn page_turn_first_placement_override_keeps_saved_global_and_book_values_and_other_readers() {
    use crate::settings::SingletonSpreadPlacementPreference::{Center, FollowGlobal, Place};
    for mode in [SpreadMode::LtrCover, SpreadMode::RtlCover] {
        for saved_first in [false, true] {
            for first_preference in [FollowGlobal, Place, Center] {
                let ctx = egui::Context::default();
                let mut app = book(&ctx, mode, 12, saved_first, true, false);
                app.singleton_spread_endpoint_preferences.first = first_preference;
                app.singleton_spread_endpoint_preferences.last = Center;
                let saved_preferences = app.singleton_spread_endpoint_preferences;
                let saved_effective_first = first_preference.effective(saved_first);
                let saved_side = if saved_effective_first {
                    cover_side(mode)
                } else {
                    SingletonSpreadPlacement::Center
                };
                for enabled in [false, true, false] {
                    let mut settings = app.settings.clone();
                    settings.page_turn_animation_enabled = enabled;
                    app.install_preferences_settings(settings);
                    app.reading_flow = ReadingFlow::Paged;
                    let expected = if enabled {
                        cover_side(mode)
                    } else {
                        saved_side
                    };
                    assert_eq!(
                        app.spread_display_composition_for_anchor(0)
                            .singleton_placement(),
                        expected
                    );
                    frame(&ctx, |ui, ctx| {
                        app.begin_page_turn_viewer_frame(ctx, false, None);
                        draw_observed(&mut app, ui, ctx);
                        assert_eq!(
                            actual_side(
                                &app.fullscreen_page_layout.single_page().unwrap().transform
                            ),
                            expected
                        );
                    });
                    assert_eq!(app.settings.singleton_spread_first_enabled, saved_first);
                    assert!(app.settings.singleton_spread_last_enabled);
                    assert_eq!(app.singleton_spread_endpoint_preferences, saved_preferences);
                    let saved_endpoints = app.singleton_spread_placement_enabled_for_current_book();
                    assert_eq!(saved_endpoints.first, saved_effective_first);
                    assert!(!saved_endpoints.last);

                    let remote = build_remote_spread_page_groups_with_shift(
                        &app.items,
                        mode,
                        &(0..12)
                            .map(|idx| [3, 6, 7].contains(&idx))
                            .collect::<Vec<_>>(),
                        false,
                        saved_endpoints,
                        crate::settings::PageAloneSettings::default(),
                        true,
                        None,
                    );
                    assert_eq!(remote[0].indices, vec![0]);
                    assert_eq!(remote[0].singleton_placement, saved_side);
                    for flow in [ReadingFlow::Vertical, ReadingFlow::Horizontal] {
                        app.reading_flow = flow;
                        assert!(app.continuous_reading_active_for_idx(0));
                        assert_eq!(
                            app.spread_display_composition_for_anchor(0)
                                .singleton_placement(),
                            saved_side
                        );
                        let (units, pos) = app.continuous_reading_units_and_pos(0).unwrap();
                        assert_eq!(pos, 0);
                        assert_eq!(units[0].anchor_idx, 0);
                        assert_eq!(units[0].singleton_placement, saved_side);
                    }
                    app.reading_flow = ReadingFlow::Paged;
                }
            }
        }
    }
}

#[test]
fn page_turn_cover_cancel_to_off_repaints_saved_center_before_completing() {
    for mode in [SpreadMode::LtrCover, SpreadMode::RtlCover] {
        let ctx = egui::Context::default();
        let mut app = book(&ctx, mode, 12, false, false, false);
        capture(&ctx, &mut app, 0);
        capture(&ctx, &mut app, 1);
        let operation = ViewerOperationId::Slideshow(next_operation_batch());
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            app.page_turn_external_operation(
                ctx,
                operation,
                OperationPhase::Press,
                "cover-off-regression",
            );
            app.handle_fs_navigation_with_operation(
                ctx,
                false,
                false,
                None,
                None,
                None,
                FsPageNav::Target(0),
                None,
                1,
                Some(operation),
            );
            let sources = draw_observed(&mut app, ui, ctx);
            let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                panic!("turning cover");
            };
            assert!(receipt.curl);
        });
        let (tracker, old_turn_unit, incoming_bytes) = match &app.page_turn_presentation.state {
            PresentationState::Turning {
                incoming, offer, ..
            } => {
                assert_eq!(actual_side(&incoming.pages[0].transform), cover_side(mode));
                assert_eq!(offer.submission.status(), SubmissionStatus::Started);
                assert_eq!(
                    incoming.reserved_bytes.load(Ordering::Relaxed),
                    incoming.retained_bytes()
                );
                assert!(incoming.retained_bytes() > 0);
                (
                    offer.submission.clone(),
                    Arc::downgrade(incoming),
                    incoming.retained_bytes(),
                )
            }
            _ => panic!("adopted cover turn"),
        };
        let mut settings = app.settings.clone();
        settings.page_turn_animation_enabled = false;
        app.install_preferences_settings(settings);
        assert_eq!(tracker.status(), SubmissionStatus::Cancelled);
        assert!(app.page_turn_completion_active());
        assert!(
            old_turn_unit.upgrade().is_none(),
            "the original incoming Arc has retired"
        );
        let settings_unit = match &app.page_turn_presentation.state {
            PresentationState::Completing { incoming, .. } => {
                assert_eq!(
                    incoming.reserved_bytes.load(Ordering::Relaxed),
                    incoming_bytes
                );
                assert_eq!(
                    app.page_turn_leases.reserved_bytes(),
                    incoming.retained_bytes(),
                    "settings replacement remains in the weak reservation inventory after the old Arc retires"
                );
                let mut accountant = crate::vram_budget::VramAccountant::default();
                app.page_turn_leases.account(&mut accountant);
                assert_eq!(accountant.finish().total_bytes(), incoming.retained_bytes());
                Arc::downgrade(incoming)
            }
            _ => panic!("settings replacement completion"),
        };
        assert!(!app.settings.singleton_spread_first_enabled);
        assert_eq!(
            app.spread_display_composition_for_anchor(0)
                .singleton_placement(),
            SingletonSpreadPlacement::Center
        );
        frame(&ctx, |ui, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
            let sources = draw_observed(&mut app, ui, ctx);
            let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                panic!("new-setting canonical flat");
            };
            assert!(!receipt.curl);
            assert_eq!(
                actual_side(&app.fullscreen_page_layout.single_page().unwrap().transform),
                SingletonSpreadPlacement::Center
            );
            let PresentationState::Completing { incoming, .. } = &app.page_turn_presentation.state
            else {
                panic!("completion retains its current geometry until adoption");
            };
            assert_eq!(
                actual_side(&incoming.pages[0].transform),
                SingletonSpreadPlacement::Center
            );
        });
        assert!(
            settings_unit.upgrade().is_none(),
            "normal Center preparation replaces the settings Arc"
        );
        let PresentationState::Completing { incoming, .. } = &app.page_turn_presentation.state
        else {
            panic!("adopted Center flat awaiting completion");
        };
        assert_eq!(
            incoming.reserved_bytes.load(Ordering::Relaxed),
            incoming_bytes
        );
        assert_eq!(
            app.page_turn_leases.reserved_bytes(),
            incoming.retained_bytes(),
            "the actual OFF geometry replacement remains reserved after both earlier Arcs retire"
        );
        let mut accountant = crate::vram_budget::VramAccountant::default();
        app.page_turn_leases.account(&mut accountant);
        assert_eq!(accountant.finish().total_bytes(), incoming.retained_bytes());
        frame(&ctx, |_, ctx| {
            app.begin_page_turn_viewer_frame(ctx, false, None);
        });
        assert!(!app.page_turn_completion_active());
        assert_eq!(
            app.page_turn_leases.reserved_bytes(),
            0,
            "canonical adoption releases the reservation normally"
        );
    }
}

#[test]
fn page_turn_cover_setting_change_retires_old_flat_evidence_and_commits_continuation_once() {
    for mode in [SpreadMode::LtrCover, SpreadMode::RtlCover] {
        for change_after_output in [false, true] {
            for final_enabled in [false, true] {
                let ctx = egui::Context::default();
                let mut app = book(&ctx, mode, 12, false, false, false);
                capture(&ctx, &mut app, 0);
                capture(&ctx, &mut app, 1);
                let turn = ViewerOperationId::Slideshow(next_operation_batch());
                frame(&ctx, |ui, ctx| {
                    app.begin_page_turn_viewer_frame(ctx, false, None);
                    app.page_turn_external_operation(
                        ctx,
                        turn,
                        OperationPhase::Press,
                        "cover-settings-turn",
                    );
                    app.handle_fs_navigation_with_operation(
                        ctx,
                        false,
                        false,
                        None,
                        None,
                        None,
                        FsPageNav::Target(0),
                        None,
                        1,
                        Some(turn),
                    );
                    let sources = draw_observed(&mut app, ui, ctx);
                    let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                        panic!("initial cover curl");
                    };
                    assert!(receipt.curl);
                });
                let continuation = ViewerOperationId::Slideshow(next_operation_batch());
                take_test_events();
                let mut old_receipt = None;
                let mut old_evidence = None;
                frame(&ctx, |ui, ctx| {
                    app.begin_page_turn_viewer_frame(ctx, false, None);
                    app.page_turn_external_operation(
                        ctx,
                        continuation,
                        OperationPhase::Press,
                        "cover-settings-next",
                    );
                    app.handle_fs_navigation_with_operation(
                        ctx,
                        false,
                        false,
                        None,
                        None,
                        None,
                        FsPageNav::Target(1),
                        None,
                        0,
                        Some(continuation),
                    );
                    assert_eq!(app.fullscreen_idx, Some(0));
                    let sources = draw_observed(&mut app, ui, ctx);
                    let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                        panic!("canonical flat with deferred continuation");
                    };
                    assert!(!receipt.curl);
                    assert!(!receipt.was_accepted(), "output hook has not run");
                    let PresentationState::Completing {
                        incoming,
                        continuation: command,
                        ..
                    } = &app.page_turn_presentation.state
                    else {
                        panic!("owned continuation");
                    };
                    assert_eq!(command.as_ref().unwrap().fact.operation_id, continuation);
                    assert_eq!(actual_side(&incoming.pages[0].transform), cover_side(mode));
                    old_receipt = Some(receipt);
                    old_evidence = Some(incoming.evidence.clone());
                    if !change_after_output {
                        adopt_off_then_optional_on(&mut app, final_enabled);
                    }
                });
                let old_receipt = old_receipt.unwrap();
                assert!(old_receipt.was_accepted());
                if change_after_output {
                    adopt_off_then_optional_on(&mut app, final_enabled);
                }
                let PresentationState::Completing {
                    incoming,
                    completion,
                    continuation: command,
                    ..
                } = &app.page_turn_presentation.state
                else {
                    panic!("settings adoption must retain its resolved continuation");
                };
                assert!(matches!(completion, FlatCompletion::NeedFlat));
                assert_eq!(command.as_ref().unwrap().fact.operation_id, continuation);
                assert!(
                    !incoming.evidence.was_accepted(),
                    "old output adoption cannot accept the replacement evidence"
                );
                let AcceptanceEvidence::Deferred(old) = old_evidence.unwrap() else {
                    panic!("old deferred output evidence");
                };
                let AcceptanceEvidence::Deferred(new) = &incoming.evidence else {
                    panic!("replacement deferred output evidence");
                };
                assert!(!Arc::ptr_eq(&old, new));
                assert!(app.pending_fs_page_turn_ready.is_none());
                frame(&ctx, |ui, ctx| {
                    app.begin_page_turn_viewer_frame(ctx, false, None);
                    assert_eq!(app.fullscreen_idx, Some(0));
                    assert!(!app.commit_page_turn_continuation(ctx));
                    let sources = draw_observed(&mut app, ui, ctx);
                    let FsDisplayUnitSubmission::Paged(receipt) = sources.submission else {
                        panic!("replacement canonical flat");
                    };
                    assert!(!receipt.curl);
                    assert!(!receipt.was_accepted());
                    assert_eq!(
                        actual_side(&app.fullscreen_page_layout.single_page().unwrap().transform),
                        if final_enabled {
                            cover_side(mode)
                        } else {
                            SingletonSpreadPlacement::Center
                        }
                    );
                    assert!(
                        !app.commit_page_turn_continuation(ctx),
                        "new output has not been adopted"
                    );
                });
                frame(&ctx, |_, ctx| {
                    app.begin_page_turn_viewer_frame(ctx, false, None);
                    assert_eq!(app.fullscreen_idx, Some(1));
                    assert!(
                        !app.commit_page_turn_continuation(ctx),
                        "one retained command is consumed once"
                    );
                });
                assert!(!app.page_turn_completion_active());
                let events: Vec<_> = take_test_events()
                    .into_iter()
                    .filter(|(fact, _, _)| fact.operation_id == continuation)
                    .collect();
                assert_eq!(
                    events
                        .iter()
                        .filter(|(_, kind, reason)| *kind == "skip"
                            && *reason == "stage1_continuation_instant")
                        .count(),
                    1,
                    "{events:?}"
                );
            }
        }
    }
}
