//! Handler regressions for composition permission after a manual endpoint wrap.
use super::*;
use crate::{
    app::{AppTestEnvForTest, file_type_resume::PreparedPageIdentityIndex},
    book_composition::fixtures::document_page,
    fs_animation::{FsCacheEntry, StaticAnimationState},
    grid_item::{GridItem, ThumbnailState},
    page_split::PageSlice,
    settings::SpreadMode,
    ui_fullscreen::FsPageNav,
};
use std::{sync::Arc, time::Duration};

const WAIT: Duration = Duration::from_secs(30);

fn fixture(
    mode: SpreadMode,
    hidden: &[u32],
    portrait: Option<usize>,
) -> (
    AppTestEnvForTest,
    egui::Context,
    BookCompositionProcessOwner,
) {
    let mut env = crate::app::setup_app_for_test();
    let root = env.tmp.path().join("manual-wrap-permission.pdf");
    std::fs::write(&root, b"fixture: all page textures are already cached").unwrap();
    env.items = (0..4)
        .map(|page_num| GridItem::PdfPage {
            pdf_path: root.clone(),
            page_num,
            content_type: None,
        })
        .collect();
    env.thumbnails = vec![ThumbnailState::Pending; 4];
    env.image_metas = vec![None; 4];
    env.visible_indices = (0..4).collect();
    env.items_generation += 1;
    // An installed mixed/extracted list retains PDF occurrences even when their
    // source composition hides a page. Permission must skip them at open time.
    env.current_folder = None;
    env.projection_recipe = crate::app::book_materialization::ProjectionRecipe::OrdinaryList;
    env.page_identity_index = Arc::new(PreparedPageIdentityIndex::prepare(
        &env.items,
        Some(&root),
        env.settings.file_type_visibility.clone(),
    ));
    env.settings.fullscreen_manual_nav_wrap = true;
    env.settings.page_turn_animation_enabled = false;
    env.spread_mode = mode;
    let ctx = egui::Context::default();
    crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
    for idx in 0..4 {
        let dims = if mode == SpreadMode::Single || mode.is_spread() || portrait == Some(idx) {
            [1000, 2000]
        } else {
            [2000, 1000]
        };
        let pixels = egui::ColorImage::filled([2, 2], egui::Color32::WHITE);
        let tex = ctx.load_texture(
            format!("manual-wrap-page-{idx}"),
            pixels.clone(),
            egui::TextureOptions::LINEAR,
        );
        env.fs_cache.insert(
            idx,
            FsCacheEntry::Static {
                tex,
                pixels: Arc::new(pixels),
                source_dims: Some(dims),
                load_seq: 0,
                animation: StaticAnimationState::Still,
            },
        );
        env.record_page_dims_for_spread(idx, (dims[0] as u32, dims[1] as u32));
        env.rotation_cache
            .insert(idx, crate::rotation_db::Rotation::None);
    }
    let (service, owner) =
        BookCompositionService::spawn_at(env.tmp.path().join("permission-data"), None).unwrap();
    let scope = crate::book_scope::BookScopeKey::document(&root).unwrap();
    let mut record =
        BookComposition::new(ReadingDirection::Ltr, (0..4).map(document_page).collect());
    record.hidden_pages = hidden.iter().copied().map(document_page).collect();
    service
        .save(scope, PageDomain::Document, record)
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    env.install_book_composition_runtime(service);
    (env, ctx, owner)
}

fn run_pass(ctx: &egui::Context, action: impl FnMut(&egui::Context)) {
    ctx.run_pass(egui::RawInput::default(), action)
        .drop_without_applying_deltas();
}

fn navigate(env: &mut App, ctx: &egui::Context, current: usize, delta: i32) -> FsPageNav {
    let nav = env.spread_page_nav(delta);
    run_pass(ctx, |ctx| {
        env.handle_fs_navigation(ctx, false, false, None, None, None, nav, None, current)
    });
    nav
}

fn assert_checking(env: &App, idx: usize, purpose: BookTargetPurpose) {
    let OpenPermission::Checking { intent, candidates } = &env.book_composition_permission.open
    else {
        panic!("expected a pending permission for source {idx}");
    };
    assert_eq!(intent.idx, idx);
    assert_eq!(intent.purpose, purpose);
    assert!(matches!(
        intent.route,
        BookTargetRoute::FullscreenNavigation
    ));
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.occurrence().item_index == idx)
    );
}

/// Await exactly this worker episode, then hold its replies outside the owner.
/// An empty poll below therefore exercises Checking without a scheduling race.
fn hold_current_completion(
    env: &mut App,
) -> Vec<(
    mpsc::Sender<CandidatePermissionReply>,
    CandidatePermissionReply,
)> {
    let state = std::mem::take(&mut env.book_composition_permission.open);
    let OpenPermission::Checking {
        intent,
        mut candidates,
    } = state
    else {
        panic!("expected Checking before delivering a worker episode");
    };
    let mut held = Vec::new();
    for candidate in &mut candidates {
        let OpenCandidateState::Checking(receiver) = &candidate.state else {
            panic!("expected a new candidate receiver");
        };
        let reply = receiver.recv_timeout(WAIT).unwrap();
        assert_eq!(reply.correlation, candidate.correlation);
        let (deliver, receiver) = mpsc::channel();
        candidate.state = OpenCandidateState::Checking(receiver);
        held.push((deliver, reply));
    }
    env.book_composition_permission.open = OpenPermission::Checking { intent, candidates };
    held
}

fn complete_current(env: &mut App, ctx: &egui::Context) {
    for (deliver, reply) in hold_current_completion(env) {
        deliver.send(reply).unwrap();
    }
    run_pass(ctx, |ctx| env.poll_book_presentation_permission(ctx));
}

#[test]
fn manual_nav_wrap_hidden_single_page_keeps_input_direction_after_checking() {
    for (current, delta, hidden, expected, purpose) in [
        (3, 1, [0, 1], 2, BookTargetPurpose::Next),
        (0, -1, [3, 2], 1, BookTargetPurpose::Previous),
    ] {
        let (mut env, ctx, owner) = fixture(SpreadMode::Single, &hidden, None);
        env.fullscreen_idx = Some(current);
        env.selected = Some(current);
        env.fullscreen_page_slice = PageSlice::Full;
        assert!(matches!(
            navigate(&mut env, &ctx, current, delta),
            FsPageNav::Delta(_)
        ));
        assert_checking(&env, hidden[0] as usize, purpose);
        let held = hold_current_completion(&mut env);
        run_pass(&ctx, |ctx| env.poll_book_presentation_permission(ctx));
        assert_checking(&env, hidden[0] as usize, purpose);
        assert_eq!(env.fullscreen_idx, Some(current));
        assert_eq!(env.selected, Some(current));
        for (deliver, reply) in held {
            assert!(matches!(&reply.status, Ok(CandidatePermission::Hidden(_))));
            deliver.send(reply).unwrap();
        }
        run_pass(&ctx, |ctx| env.poll_book_presentation_permission(ctx));
        assert_checking(&env, hidden[1] as usize, purpose);
        complete_current(&mut env, &ctx);
        assert_checking(&env, expected, purpose);
        complete_current(&mut env, &ctx);
        assert_eq!(env.fullscreen_idx, Some(expected));
        assert_eq!(env.selected, Some(expected));
        assert!(env.fs_boundary_hint.is_none());
        owner.shutdown_and_join();
    }
}

#[test]
fn manual_nav_wrap_hidden_split_skips_sources_for_ltr_rtl_and_nonwrapped_steps() {
    for mode in [SpreadMode::SplitLtr, SpreadMode::SplitRtl] {
        for wrap in [false, true] {
            for forward in [false, true] {
                // A portrait origin leaves slice Full, exercising the same-source
                // opposite-half skip. A split origin also covers a different-source
                // Split result. Neither is allowed to read the hidden second half.
                for portrait_origin in [false, true] {
                    let current = if forward == wrap { 3 } else { 0 };
                    let hidden = if forward {
                        if wrap { 0 } else { 1 }
                    } else if wrap {
                        3
                    } else {
                        2
                    };
                    let expected = if forward { hidden + 1 } else { hidden - 1 };
                    let (mut env, ctx, owner) =
                        fixture(mode, &[hidden as u32], portrait_origin.then_some(current));
                    env.settings.fullscreen_manual_nav_wrap = wrap;
                    env.fullscreen_idx = Some(current);
                    env.selected = Some(current);
                    let first_half = if mode == SpreadMode::SplitLtr {
                        PageSlice::Left
                    } else {
                        PageSlice::Right
                    };
                    let last_half = if mode == SpreadMode::SplitLtr {
                        PageSlice::Right
                    } else {
                        PageSlice::Left
                    };
                    env.fullscreen_page_slice = if portrait_origin {
                        PageSlice::Full
                    } else if forward {
                        last_half
                    } else {
                        first_half
                    };
                    let purpose = if forward {
                        BookTargetPurpose::Next
                    } else {
                        BookTargetPurpose::Previous
                    };
                    let nav = navigate(&mut env, &ctx, current, if forward { 1 } else { -1 });
                    if wrap {
                        assert!(
                            matches!(nav, FsPageNav::Wrap(target) if target.idx == hidden && target.forward == forward)
                        );
                    } else {
                        assert!(matches!(nav, FsPageNav::Split(step) if step.source_idx == hidden));
                    }
                    assert_checking(&env, hidden, purpose);
                    complete_current(&mut env, &ctx);
                    assert_checking(&env, expected, purpose);
                    assert_eq!(env.fullscreen_idx, Some(current));
                    complete_current(&mut env, &ctx);
                    assert_eq!(env.fullscreen_idx, Some(expected));
                    assert_eq!(env.selected, Some(expected));
                    owner.shutdown_and_join();
                }
            }
        }
    }
}

#[test]
fn manual_nav_wrap_all_hidden_continuation_stops_at_source_edge_without_rewrapping() {
    for (current, delta, order, purpose) in [
        (3, 1, [0, 1, 2, 3], BookTargetPurpose::Next),
        (0, -1, [3, 2, 1, 0], BookTargetPurpose::Previous),
    ] {
        let (mut env, ctx, owner) = fixture(SpreadMode::Single, &[0, 1, 2, 3], None);
        env.fullscreen_idx = Some(current);
        env.selected = Some(current);
        navigate(&mut env, &ctx, current, delta);
        for idx in order {
            assert_checking(&env, idx, purpose);
            complete_current(&mut env, &ctx);
        }
        assert_eq!(env.fullscreen_idx, None);
        assert!(matches!(
            env.book_composition_permission.open,
            OpenPermission::Idle
        ));
        owner.shutdown_and_join();
    }
}

fn assert_unit_checking(env: &mut App, idx: usize, purpose: BookTargetPurpose) {
    let demand = env.book_target_presentation_demand(idx);
    let OpenPermission::Checking { intent, candidates } = &env.book_composition_permission.open
    else {
        panic!("expected pending permission for unit at {idx}");
    };
    assert_eq!(intent.idx, idx);
    assert_eq!(intent.purpose, purpose);
    assert!(matches!(
        intent.route,
        BookTargetRoute::FullscreenNavigation
    ));
    assert!(candidates.iter().map(OpenCandidate::occurrence).eq(demand));
}

fn assert_hidden_wrap_returns_to_origin_boundary(
    mode: SpreadMode,
    current: usize,
    allowed: &[usize],
    forward: bool,
    slice: PageSlice,
) {
    let hidden = (0..4)
        .filter(|idx| !allowed.contains(idx))
        .map(|idx| idx as u32)
        .collect::<Vec<_>>();
    let (mut env, ctx, owner) = fixture(mode, &hidden, None);
    env.fullscreen_idx = Some(current);
    env.selected = Some(current);
    env.fullscreen_page_slice = slice;
    let purpose = if forward {
        BookTargetPurpose::Next
    } else {
        BookTargetPurpose::Previous
    };
    navigate(&mut env, &ctx, current, if forward { 1 } else { -1 });
    let mut finished = false;
    for _ in 0..8 {
        let OpenPermission::Checking { intent, .. } = &env.book_composition_permission.open else {
            panic!("hidden traversal must await its next candidate or finish at the boundary");
        };
        let target = intent.idx;
        assert_unit_checking(&mut env, target, purpose);
        // Keep each completion outside the owner for one poll. Neither the
        // original wrap nor a Hidden skip may move the shown unit while Checking.
        let held = hold_current_completion(&mut env);
        run_pass(&ctx, |ctx| env.poll_book_presentation_permission(ctx));
        assert_unit_checking(&mut env, target, purpose);
        assert_eq!(env.fullscreen_idx, Some(current));
        assert_eq!(env.fullscreen_page_slice, slice);
        for (deliver, reply) in held {
            deliver.send(reply).unwrap();
        }
        run_pass(&ctx, |ctx| env.poll_book_presentation_permission(ctx));
        assert_eq!(env.fullscreen_idx, Some(current));
        assert_eq!(env.selected, Some(current));
        assert_eq!(env.fullscreen_page_slice, slice);
        if let Some(crate::ui_fullscreen::FsBoundaryHint::Edge { at_end, .. }) =
            env.fs_boundary_hint
        {
            assert_eq!(at_end, forward);
            assert!(matches!(
                env.book_composition_permission.open,
                OpenPermission::Idle
            ));
            finished = true;
            break;
        }
    }
    assert!(
        finished,
        "no other readable unit must end at the input boundary"
    );
    owner.shutdown_and_join();
}

#[test]
fn manual_nav_wrap_hidden_single_page_return_to_origin_is_boundary_after_checking() {
    for forward in [false, true] {
        let current = if forward { 3 } else { 0 };
        assert_hidden_wrap_returns_to_origin_boundary(
            SpreadMode::Single,
            current,
            &[current],
            forward,
            PageSlice::Full,
        );
    }
}

#[test]
fn manual_nav_wrap_hidden_spread_return_to_origin_unit_is_boundary_after_checking() {
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        for forward in [false, true] {
            let origin = if forward { [2, 3] } else { [0, 1] };
            // The shown anchor may be either real page in the same unit.
            for current in origin {
                assert_hidden_wrap_returns_to_origin_boundary(
                    mode,
                    current,
                    &origin,
                    forward,
                    PageSlice::Full,
                );
            }
        }
    }
}

#[test]
fn manual_nav_wrap_hidden_split_return_to_origin_source_is_boundary_after_checking() {
    for mode in [SpreadMode::SplitLtr, SpreadMode::SplitRtl] {
        for forward in [false, true] {
            let current = if forward { 3 } else { 0 };
            let slice = match (mode, forward) {
                (SpreadMode::SplitLtr, true) | (SpreadMode::SplitRtl, false) => PageSlice::Right,
                _ => PageSlice::Left,
            };
            assert_hidden_wrap_returns_to_origin_boundary(
                mode,
                current,
                &[current],
                forward,
                slice,
            );
        }
    }
}

#[test]
fn manual_nav_wrap_hidden_spread_origin_survivor_is_boundary_after_checking() {
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        for forward in [false, true] {
            let current = if forward { 2 } else { 0 };
            // The source anchor is now Hidden, but its permitted partner still
            // belongs to the unit shown when the manual wrap started.
            assert_hidden_wrap_returns_to_origin_boundary(
                mode,
                current,
                &[current + 1],
                forward,
                PageSlice::Full,
            );
        }
    }
}

#[test]
fn manual_nav_wrap_nonwrapped_same_unit_permission_keeps_navigation() {
    let (mut env, ctx, owner) = fixture(SpreadMode::Ltr, &[], None);
    env.fullscreen_idx = Some(1);
    env.selected = Some(1);
    run_pass(&ctx, |ctx| {
        env.handle_fs_navigation(
            ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNav::Target(0),
            None,
            1,
        );
    });
    assert_unit_checking(&mut env, 0, BookTargetPurpose::Previous);
    let OpenPermission::Checking { intent, .. } = &env.book_composition_permission.open else {
        unreachable!();
    };
    assert!(
        intent.navigation_origin.is_empty(),
        "ordinary admission must not own a wrap origin"
    );
    complete_current(&mut env, &ctx);
    assert_eq!(env.fullscreen_idx, Some(0));
    assert!(env.fs_boundary_hint.is_none());
    owner.shutdown_and_join();
}

fn hide_every_page_except(env: &mut App, current: usize) {
    let GridItem::PdfPage { pdf_path, .. } = &env.items[current] else {
        unreachable!();
    };
    let scope = crate::book_scope::BookScopeKey::document(pdf_path).unwrap();
    let mut record =
        BookComposition::new(ReadingDirection::Ltr, (0..4).map(document_page).collect());
    record.hidden_pages = (0..4)
        .filter(|idx| *idx != current as u32)
        .map(document_page)
        .collect();
    env.book_composition_service
        .as_ref()
        .unwrap()
        .save(scope, PageDomain::Document, record)
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
}

fn finish_origin_recheck(env: &mut App, ctx: &egui::Context, current: usize, forward: bool) {
    let purpose = if forward {
        BookTargetPurpose::Next
    } else {
        BookTargetPurpose::Previous
    };
    for target in if forward {
        vec![0, 1, 2, 3]
    } else {
        vec![3, 2, 1, 0]
    } {
        let OpenPermission::Checking { intent, .. } = &env.book_composition_permission.open else {
            panic!("stale continuation must recheck source {target}");
        };
        assert_eq!(intent.idx, target);
        assert_eq!(intent.purpose, purpose);
        assert_eq!(intent.navigation_origin.len(), 1);
        complete_current(env, ctx);
        assert_eq!(env.fullscreen_idx, Some(current));
        assert_eq!(env.fullscreen_page_slice, PageSlice::Full);
    }
    assert!(
        matches!(env.fs_boundary_hint, Some(crate::ui_fullscreen::FsBoundaryHint::Edge { at_end, .. }) if at_end == forward)
    );
    assert!(matches!(
        env.book_composition_permission.open,
        OpenPermission::Idle
    ));
}

#[test]
fn manual_nav_wrap_origin_survives_stale_permission_recheck() {
    for flow in [
        crate::settings::ReadingFlow::Paged,
        crate::settings::ReadingFlow::Vertical,
    ] {
        for forward in [false, true] {
            let current = if forward { 3 } else { 0 };
            let (mut env, ctx, owner) = fixture(SpreadMode::Single, &[], None);
            env.fullscreen_idx = Some(current);
            env.selected = Some(current);
            env.reading_flow = flow;
            navigate(&mut env, &ctx, current, if forward { 1 } else { -1 });
            // Stop precisely between qualification and its internal callback,
            // then invalidate that proof through the real composition service.
            let replies = hold_current_completion(&mut env);
            let OpenPermission::Checking {
                intent,
                mut candidates,
            } = std::mem::take(&mut env.book_composition_permission.open)
            else {
                unreachable!();
            };
            for (candidate, (_, reply)) in candidates.iter_mut().zip(replies) {
                let permission = reply.status.unwrap();
                assert!(matches!(permission, CandidatePermission::Allowed(_)));
                candidate.state = OpenCandidateState::Complete(permission);
            }
            env.publish_qualified_book_candidates(&candidates);
            env.book_composition_permission.open = OpenPermission::Resuming {
                intent: intent.clone(),
                candidates,
            };
            hide_every_page_except(&mut env, current);
            run_pass(&ctx, |ctx| env.resume_qualified_book_target(ctx, &intent));
            finish_origin_recheck(&mut env, &ctx, current, forward);
            owner.shutdown_and_join();
        }
    }
}

#[test]
fn manual_nav_wrap_continuous_inline_permission_keeps_origin_until_commit() {
    use crate::app::book_composition_runtime::{BookCompositionSource, ViewerBookComposition};
    use crate::fs_page_load_scheduler::FsPageLoadContract;
    for flow in [
        crate::settings::ReadingFlow::Vertical,
        crate::settings::ReadingFlow::Horizontal,
    ] {
        for forward in [false, true] {
            let current = if forward { 3 } else { 0 };
            let target = if forward { 0 } else { 3 };
            let purpose = if forward {
                BookTargetPurpose::Next
            } else {
                BookTargetPurpose::Previous
            };
            let inferred = if forward {
                BookTargetPurpose::Previous
            } else {
                BookTargetPurpose::Next
            };
            let (mut env, _ctx, owner) = fixture(SpreadMode::Single, &[], None);
            let GridItem::PdfPage { pdf_path: root, .. } = env.items[0].clone() else {
                unreachable!();
            };
            let scope = crate::book_scope::BookScopeKey::document(&root).unwrap();
            let prepared = env
                .book_composition_service
                .as_ref()
                .unwrap()
                .prepare(scope.clone(), PageDomain::Document)
                .recv_timeout(WAIT)
                .unwrap();
            let record = prepared.status.unwrap().map(Arc::new);
            let rows = crate::app::book_materialization::project_document_composition(
                &root,
                4,
                (env.items.clone(), vec![None; 4]),
                env.settings.file_type_visibility.clone(),
                prepared.token,
                record.as_ref().map(|record| &record.composition),
            )
            .unwrap();
            env.page_identity_index = rows.page_identities;
            env.book_composition = ViewerBookComposition::Accepted {
                scope,
                token: prepared.token,
                record,
                layout: rows.composition,
                source: BookCompositionSource::Document {
                    count: 4,
                    metadata: None,
                    physical_source: root,
                    read_target: None,
                },
                generation: env.items_generation,
                cause: Default::default(),
            };
            env.fullscreen_idx = Some(current);
            env.reading_flow = flow;
            let origin = env.capture_book_navigation_origin();
            assert!(!env.defer_book_composition_wrapped_navigation(
                target,
                FsPageLoadContract::Sequential,
                purpose,
                origin
            ));
            assert!(
                matches!(&env.book_composition_permission.open, OpenPermission::Resuming { intent, .. } if intent.navigation_origin.len() == 1 && intent.route == BookTargetRoute::ContinuousSeek)
            );
            // The existing seek commit consumes this proof, without starting a
            // new episode or changing the ordinary empty-origin path.
            assert!(!env.defer_book_composition_continuous_seek(
                target,
                FsPageLoadContract::Sequential,
                inferred
            ));
            assert!(matches!(
                env.book_composition_permission.open,
                OpenPermission::Idle
            ));
            let origin = env.capture_book_navigation_origin();
            assert!(!env.defer_book_composition_wrapped_navigation(
                target,
                FsPageLoadContract::Sequential,
                purpose,
                origin
            ));
            hide_every_page_except(&mut env, current);
            assert!(env.defer_book_composition_continuous_seek(
                target,
                FsPageLoadContract::Sequential,
                inferred
            ));
            finish_origin_recheck(&mut env, &_ctx, current, forward);
            owner.shutdown_and_join();
        }
    }
}

fn install_mixed_image_rows(env: &mut AppTestEnvForTest, pages: &[Option<u32>]) {
    let GridItem::PdfPage { pdf_path: root, .. } = env.items[0].clone() else {
        unreachable!();
    };
    let image_path = env.tmp.path().join("ordinary-origin.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([255; 4]))
        .save(&image_path)
        .unwrap();
    env.items = pages
        .iter()
        .map(|page| match page {
            Some(page_num) => GridItem::PdfPage {
                pdf_path: root.clone(),
                page_num: *page_num,
                content_type: None,
            },
            None => GridItem::Image(image_path.clone()),
        })
        .collect();
    env.thumbnails = vec![ThumbnailState::Pending; pages.len()];
    env.image_metas = vec![None; pages.len()];
    env.visible_indices = (0..pages.len()).collect();
    env.items_generation += 1;
    env.page_identity_index = Arc::new(PreparedPageIdentityIndex::prepare(
        &env.items,
        None,
        env.settings.file_type_visibility.clone(),
    ));
    env.viewer_navigation_caches.invalidate();
    env.fs_cache.retain(|idx, _| *idx < pages.len());
}

#[test]
fn manual_nav_wrap_hidden_pdf_returns_to_plain_image_origin_at_boundary() {
    use crate::settings::ReadingFlow;
    for flow in [
        ReadingFlow::Paged,
        ReadingFlow::Vertical,
        ReadingFlow::Horizontal,
    ] {
        for forward in [false, true] {
            let (mut env, ctx, owner) = fixture(SpreadMode::Single, &[0], None);
            let pages = if forward {
                [Some(0), None]
            } else {
                [None, Some(0)]
            };
            install_mixed_image_rows(&mut env, &pages);
            let current = usize::from(forward);
            env.reading_flow = flow;
            env.fullscreen_idx = Some(current);
            env.selected = Some(current);
            env.fullscreen_page_slice = PageSlice::Full;
            navigate(&mut env, &ctx, current, if forward { 1 } else { -1 });
            assert!(
                matches!(&env.book_composition_permission.open, OpenPermission::Checking { intent, .. }
                if intent.idx == 1 - current && intent.navigation_origin.len() == 1)
            );
            complete_current(&mut env, &ctx);
            assert_eq!(env.fullscreen_idx, Some(current));
            assert_eq!(env.selected, Some(current));
            assert_eq!(env.fullscreen_page_slice, PageSlice::Full);
            assert!(
                matches!(env.fs_boundary_hint, Some(crate::ui_fullscreen::FsBoundaryHint::Edge { at_end, .. }) if at_end == forward)
            );
            assert!(
                env.fs_feedback_toast.is_none(),
                "a valid ordinary image is not a missing page"
            );
            assert!(matches!(
                env.book_composition_permission.open,
                OpenPermission::Idle
            ));
            owner.shutdown_and_join();
        }
    }
}

#[test]
fn manual_nav_wrap_nonwrapped_hidden_pdf_lands_on_plain_image() {
    use crate::settings::ReadingFlow;
    for flow in [
        ReadingFlow::Paged,
        ReadingFlow::Vertical,
        ReadingFlow::Horizontal,
    ] {
        for wrap in [false, true] {
            for forward in [false, true] {
                let (mut env, ctx, owner) = fixture(SpreadMode::Single, &[1], None);
                let pages = if forward {
                    [Some(0), Some(1), None]
                } else {
                    [None, Some(1), Some(0)]
                };
                install_mixed_image_rows(&mut env, &pages);
                let current = if forward { 0 } else { 2 };
                let target = 2 - current;
                env.settings.fullscreen_manual_nav_wrap = wrap;
                env.reading_flow = flow;
                env.fullscreen_idx = Some(current);
                env.selected = Some(current);
                navigate(&mut env, &ctx, current, if forward { 1 } else { -1 });
                assert!(
                    matches!(&env.book_composition_permission.open, OpenPermission::Checking { intent, .. }
                    if intent.idx == 1 && intent.navigation_origin.is_empty())
                );
                complete_current(&mut env, &ctx);
                assert_eq!(env.fullscreen_idx, Some(target));
                assert_eq!(env.selected, Some(target));
                assert!(env.fs_boundary_hint.is_none());
                assert!(
                    env.fs_feedback_toast.is_none(),
                    "Hidden skip must land through its ordinary route"
                );
                owner.shutdown_and_join();
            }
        }
    }
}

#[test]
fn manual_nav_wrap_permission_free_hidden_skip_keeps_other_routes_unchanged() {
    for route in [
        BookTargetRoute::FullscreenOpen(FsMetadataBoundary::Manual),
        BookTargetRoute::ZipDfs(BookDfsContinuation::Navigation { forward: true }),
        BookTargetRoute::Slideshow { slice: None },
    ] {
        let (mut env, ctx, owner) = fixture(SpreadMode::Single, &[1], None);
        install_mixed_image_rows(&mut env, &[Some(0), Some(1), None]);
        env.fullscreen_idx = Some(0);
        env.selected = Some(0);
        env.slideshow_playing = true;
        navigate(&mut env, &ctx, 0, 1);
        let OpenPermission::Checking { intent, .. } = &mut env.book_composition_permission.open
        else {
            unreachable!();
        };
        intent.route = route;
        complete_current(&mut env, &ctx);
        assert_eq!(env.fullscreen_idx, Some(0));
        assert!(env.fs_boundary_hint.is_none());
        assert!(
            matches!(&env.fs_feedback_toast, Some((message, ..)) if message == "開こうとしたページが見つかりません")
        );
        assert!(matches!(
            env.book_composition_permission.open,
            OpenPermission::Idle
        ));
        owner.shutdown_and_join();
    }
}
