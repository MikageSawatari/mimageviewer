use super::*;
use crate::app::{FsOpenMaterialization, HistoryTrigger};
use crate::fs_page_load_scheduler::FsPageLoadContract;
use crate::page_split::{PageSlice, SplitDirection};

fn split_app(ctx: &egui::Context, mode: SpreadMode) -> crate::app::AppTestEnvForTest {
    let mut app = composition_switch_app(ctx, mode, true, mode.reading_direction().unwrap());
    app.fullscreen_idx = None;
    app.open_fullscreen(0, HistoryTrigger::UserChosen);
    assert_eq!(app.fullscreen_idx, Some(0));
    app.reconcile_fullscreen_page_slice(0);
    assert_eq!(
        app.fullscreen_page_slice,
        SplitDirection::from_spread_mode(mode).unwrap().first()
    );
    app
}

fn step(app: &mut App, ctx: &egui::Context, delta: i32) {
    let current = app.fullscreen_idx.unwrap();
    let nav = app.spread_page_nav(delta);
    assert!(matches!(nav, FsPageNav::Split(_)), "{nav:?}");
    app.handle_fs_navigation(ctx, false, false, None, None, None, nav, None, current);
}

fn reopen_second_half(mode: SpreadMode, target: usize) {
    let ctx = egui::Context::default();
    let mut app = split_app(&ctx, mode);
    let direction = SplitDirection::from_spread_mode(mode).unwrap();
    step(&mut app, &ctx, 1);
    assert_eq!(app.fullscreen_page_slice, direction.second());
    app.close_fullscreen();
    assert!(app.fullscreen_idx.is_none());
    app.open_fullscreen(target, HistoryTrigger::UserChosen);
    assert_eq!(app.fullscreen_idx, Some(target));
    app.reconcile_fullscreen_page_slice(target);
    assert_eq!(app.fullscreen_page_slice, direction.first());
}

#[test]
fn split_open_rtl_reopen_different_page_starts_right() {
    reopen_second_half(SpreadMode::SplitRtl, 1);
}

#[test]
fn split_open_rtl_reopen_same_page_starts_right() {
    reopen_second_half(SpreadMode::SplitRtl, 0);
}

#[test]
fn split_open_ltr_reopen_different_page_starts_left() {
    reopen_second_half(SpreadMode::SplitLtr, 1);
}

#[test]
fn split_open_ltr_reopen_same_page_starts_left() {
    reopen_second_half(SpreadMode::SplitLtr, 0);
}

#[test]
fn split_open_closed_direction_change_from_second_half_starts_left() {
    reopen_after_direction_change(true);
}

#[test]
fn split_open_closed_direction_change_from_first_half_starts_left() {
    reopen_after_direction_change(false);
}

fn reopen_after_direction_change(start_on_second: bool) {
    let ctx = egui::Context::default();
    let mut app = split_app(&ctx, SpreadMode::SplitRtl);
    if start_on_second {
        step(&mut app, &ctx, 1);
    }
    app.close_fullscreen();
    assert!(app.apply_fullscreen_spread_mode(&ctx, 0, SpreadMode::SplitLtr));
    app.open_fullscreen(0, HistoryTrigger::UserChosen);
    app.reconcile_fullscreen_page_slice(0);
    assert_eq!(app.fullscreen_page_slice, PageSlice::Left);
}

#[test]
fn split_open_jump_without_slice_starts_at_first_half() {
    for mode in [SpreadMode::SplitRtl, SpreadMode::SplitLtr] {
        for contract in [
            FsPageLoadContract::Sequential,
            FsPageLoadContract::LatestSeek,
        ] {
            let ctx = egui::Context::default();
            let mut app = split_app(&ctx, mode);
            step(&mut app, &ctx, 1);
            app.open_fullscreen_from_fs_navigation_with_contract(
                &ctx,
                1,
                HistoryTrigger::UserChosen,
                contract,
                &mut None,
            );
            assert_eq!(app.fullscreen_idx, Some(1));
            app.reconcile_fullscreen_page_slice(1);
            assert_eq!(
                app.fullscreen_page_slice,
                SplitDirection::from_spread_mode(mode).unwrap().first(),
                "{mode:?} {contract:?}"
            );
        }
    }
}

#[test]
fn split_open_deferred_materialization_starts_at_first_half() {
    let ctx = egui::Context::default();
    let mut app = split_app(&ctx, SpreadMode::SplitRtl);
    step(&mut app, &ctx, 1);
    app.open_fullscreen_with_materialization(
        1,
        HistoryTrigger::UserChosen,
        FsOpenMaterialization::DeferredPageTurn,
    );
    assert_eq!(app.fullscreen_idx, Some(1));
    app.reconcile_fullscreen_page_slice(1);
    assert_eq!(app.fullscreen_page_slice, PageSlice::Right);
}

#[test]
fn split_open_backward_step_keeps_explicit_second_half() {
    for mode in [SpreadMode::SplitRtl, SpreadMode::SplitLtr] {
        let ctx = egui::Context::default();
        let mut app = split_app(&ctx, mode);
        app.open_fullscreen(1, HistoryTrigger::UserChosen);
        app.reconcile_fullscreen_page_slice(1);
        step(&mut app, &ctx, -1);
        assert_eq!(app.fullscreen_idx, Some(0));
        app.reconcile_fullscreen_page_slice(0);
        assert_eq!(
            app.fullscreen_page_slice,
            SplitDirection::from_spread_mode(mode).unwrap().second()
        );
    }
}

#[test]
fn split_open_same_idx_refresh_keeps_visible_half() {
    for mode in [SpreadMode::SplitRtl, SpreadMode::SplitLtr] {
        let ctx = egui::Context::default();
        let mut app = split_app(&ctx, mode);
        step(&mut app, &ctx, 1);
        let slice = app.fullscreen_page_slice;
        app.open_fullscreen(0, HistoryTrigger::UserChosen);
        app.reconcile_fullscreen_page_slice(0);
        assert_eq!(app.fullscreen_page_slice, slice);
    }
}

#[test]
fn split_open_direction_switch_on_page_keeps_visible_half() {
    for mode in [SpreadMode::SplitRtl, SpreadMode::SplitLtr] {
        for reading_direction_only in [false, true] {
            let ctx = egui::Context::default();
            let mut app = split_app(&ctx, mode);
            step(&mut app, &ctx, 1);
            let slice = app.fullscreen_page_slice;
            let direction = match app.reading_direction {
                ReadingDirection::Ltr => ReadingDirection::Rtl,
                ReadingDirection::Rtl => ReadingDirection::Ltr,
            };
            if reading_direction_only {
                app.set_reading_direction_for_fullscreen(&ctx, 0, direction);
            } else {
                assert!(app.apply_fullscreen_spread_mode(
                    &ctx,
                    0,
                    mode.with_reading_direction(direction)
                ));
            }
            app.reconcile_fullscreen_page_slice(0);
            assert_eq!(app.fullscreen_page_slice, slice);
        }
    }
}

#[test]
fn split_open_slideshow_landing_keeps_explicit_second_half() {
    let ctx = egui::Context::default();
    let mut app = split_app(&ctx, SpreadMode::SplitRtl);
    app.complete_book_slideshow_target(
        &ctx,
        1,
        Some(PageSlice::Left),
        crate::app::book_composition_permission::BookTargetPurpose::Next,
    );
    assert_eq!(app.fullscreen_idx, Some(1));
    app.reconcile_fullscreen_page_slice(1);
    assert_eq!(app.fullscreen_page_slice, PageSlice::Left);
}

#[cfg(windows)]
#[test]
fn split_open_reset_only_changes_mounted_context_slice() {
    let ctx = egui::Context::default();
    let mut app = split_app(&ctx, SpreadMode::SplitRtl);
    step(&mut app, &ctx, 1);
    let child = app.build_window_context_for_test(8747, |owner| {
        owner.items = (0..2)
            .map(|idx| GridItem::Image(PathBuf::from(format!("c:/test/split-child-{idx}.png"))))
            .collect();
        owner.visible_indices = vec![0, 1];
        owner.fullscreen_idx = Some(0);
        owner.spread_mode = SpreadMode::SplitLtr;
        owner.reading_flow = ReadingFlow::Paged;
        owner.set_fullscreen_page_slice_for_display_unit(PageSlice::Right);
    });
    app.with_window_context_for_test(child, |owner| {
        owner.open_fullscreen_with_materialization(
            1,
            HistoryTrigger::UserChosen,
            FsOpenMaterialization::DeferredPageTurn,
        );
        assert_eq!(owner.fullscreen_idx, Some(1));
        assert_eq!(owner.fullscreen_page_slice, PageSlice::Full);
    });
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.fullscreen_page_slice, PageSlice::Left);
    app.open_fullscreen(1, HistoryTrigger::UserChosen);
    app.reconcile_fullscreen_page_slice(1);
    assert_eq!(app.fullscreen_page_slice, PageSlice::Right);
    app.with_window_context_for_test(child, |owner| {
        assert_eq!(owner.fullscreen_page_slice, PageSlice::Full);
    });
}
