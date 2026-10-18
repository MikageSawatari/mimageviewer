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
        let dims = if mode == SpreadMode::Single || portrait == Some(idx) {
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
