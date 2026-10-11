//! State/handler regressions for the shared cursor/anchor owner (§1.370).
use super::*;
use crate::settings::{GridClickSelectionMode, GridViewMode, SpreadMode};
use crate::ui_fullscreen::FsPageNav;

fn grid(view: GridViewMode) -> AppTestEnvForTest {
    let mut app = setup_app_for_test();
    app.items = (0..8)
        .map(|i| GridItem::Image(app.tmp.path().join(format!("{i}.png"))))
        .collect();
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.image_metas = vec![None; app.items.len()];
    app.settings.grid_view_mode = view;
    app.settings.grid_cols = 2;
    app.settings.grid_click_selection_mode = GridClickSelectionMode::Explorer;
    app.rebuild_visible_indices();
    app.move_grid_cursor(Some(1));
    app
}

fn click(app: &mut App, target: usize, ctrl: bool, shift: bool) {
    let order = app.current_grid_order().to_vec();
    crate::ui_main::apply_grid_click_selection(
        app.settings.grid_click_selection_mode,
        &mut app.selected,
        &mut app.grid_click_selection_anchor,
        app.items_generation,
        &mut app.checked,
        &order,
        &app.items,
        false,
        target,
        ctrl,
        shift,
    );
}

fn key(app: &mut App, ctx: &egui::Context, key: egui::Key, shift: bool) {
    let modifiers = egui::Modifiers {
        shift,
        ..Default::default()
    };
    ctx.begin_pass(egui::RawInput {
        modifiers,
        events: vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }],
        ..Default::default()
    });
    let _ = app.handle_keyboard(ctx);
    let _ = ctx.end_pass();
}

fn anchor(app: &App) -> Option<usize> {
    app.grid_click_selection_anchor
        .and_then(|anchor| anchor.index_for_generation(app.items_generation))
}

#[test]
fn fullscreen_page_then_escape_then_shift_click_starts_at_returned_cursor() {
    for view in [GridViewMode::Thumbnail, GridViewMode::Details] {
        let mut app = grid(view);
        let ctx = egui::Context::default();
        // The real fullscreen navigation/close handlers, without a native window.
        app.fullscreen_idx = Some(1);
        app.handle_fs_navigation(
            &ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNav::Target(4),
            None,
            1,
        );
        assert_eq!(app.fullscreen_idx, Some(4));
        assert_eq!(app.selected, Some(4));
        assert_eq!(anchor(&app), Some(4));
        app.handle_fs_navigation(
            &ctx,
            true,
            false,
            None,
            None,
            None,
            FsPageNav::None,
            None,
            4,
        );
        assert_eq!(app.fullscreen_idx, None);
        assert_eq!(app.selected, Some(4));
        click(&mut app, 6, false, true);
        assert_eq!(app.checked, HashSet::from([4, 5, 6]));
        assert_eq!(anchor(&app), Some(4));
    }
}

#[test]
fn spread_layout_readoption_same_page_keeps_shift_click_origin() {
    for view in [GridViewMode::Thumbnail, GridViewMode::Details] {
        for mode in [SpreadMode::LtrCover, SpreadMode::RtlCover] {
            let mut app = grid(view);
            click(&mut app, 3, false, true);
            app.fullscreen_idx = Some(3);
            app.spread_mode = mode;
            // Both metadata adoption and retained re-adoption use this entry.
            for _ in 0..2 {
                app.adopt_shift_layout_without_navigation();
                assert_eq!(app.fullscreen_idx, Some(3));
                assert_eq!(app.selected, Some(3));
                assert_eq!(anchor(&app), Some(1));
                assert_eq!(app.checked, HashSet::from([1, 2, 3]));
            }
            click(&mut app, 5, false, true);
            assert_eq!(app.checked, HashSet::from([1, 2, 3, 4, 5]));
            assert_eq!(anchor(&app), Some(1));
        }
    }
}

#[test]
fn spread_layout_readoption_normalization_moves_shift_click_origin() {
    for view in [GridViewMode::Thumbnail, GridViewMode::Details] {
        for mode in [SpreadMode::LtrCover, SpreadMode::RtlCover] {
            let mut app = grid(view);
            click(&mut app, 4, false, true);
            app.fullscreen_idx = Some(4);
            app.spread_mode = mode;
            // Page 4 becomes the second page of [3, 4], so normalization
            // actually moves the cursor and must notify the shared owner.
            app.adopt_shift_layout_without_navigation();
            assert_eq!(app.fullscreen_idx, Some(3));
            assert_eq!(app.selected, Some(3));
            assert_eq!(anchor(&app), Some(3));
            assert_eq!(app.checked, HashSet::from([1, 2, 3, 4]));
            click(&mut app, 6, false, true);
            assert_eq!(app.checked, HashSet::from([3, 4, 5, 6]));
            assert_eq!(anchor(&app), Some(3));
        }
    }
}

#[test]
fn arrow_then_shift_click_uses_new_cursor_in_thumbnail_and_details_order() {
    for view in [GridViewMode::Thumbnail, GridViewMode::Details] {
        let mut app = grid(view);
        if view == GridViewMode::Details {
            app.details_order = vec![7, 1, 4, 6, 0, 2, 3, 5];
        }
        let order = app.current_grid_order().to_vec();
        let old_pos = order.iter().position(|&idx| idx == 1).unwrap();
        let target = order[old_pos + 1];
        let ctx = egui::Context::default();
        key(&mut app, &ctx, egui::Key::ArrowRight, false);
        assert_eq!(app.selected, Some(target));
        assert_eq!(anchor(&app), Some(target));
        click(&mut app, order[old_pos + 3], false, true);
        assert_eq!(
            app.checked,
            order[old_pos + 1..=old_pos + 3].iter().copied().collect()
        );
    }
}

#[test]
fn shift_arrow_keeps_anchor_for_subsequent_shift_click() {
    let mut app = grid(GridViewMode::Thumbnail);
    let ctx = egui::Context::default();
    key(&mut app, &ctx, egui::Key::ArrowRight, true);
    assert_eq!(app.selected, Some(2));
    assert_eq!(anchor(&app), Some(1));
    key(&mut app, &ctx, egui::Key::ArrowRight, true);
    assert_eq!(app.selected, Some(3));
    assert_eq!(anchor(&app), Some(1));
    click(&mut app, 5, false, true);
    assert_eq!(app.checked, HashSet::from([1, 2, 3, 4, 5]));
}

#[test]
fn page_home_end_moves_rebase_and_keep_existing_modifier_admission() {
    for nav in [
        egui::Key::PageDown,
        egui::Key::PageUp,
        egui::Key::Home,
        egui::Key::End,
    ] {
        for shift in [false, true] {
            let mut app = grid(GridViewMode::Thumbnail);
            app.move_grid_cursor(Some(3));
            app.last_cell_h = 10.0;
            app.last_viewport_h = 10.0;
            let ctx = egui::Context::default();
            key(&mut app, &ctx, nav, shift);
            if shift {
                // These default KeyActions use exact plain chords; unlike the
                // fixed Shift+arrow derivative, Shift does not admit navigation.
                assert_eq!(app.selected, Some(3), "{nav:?} with Shift");
                assert!(app.checked.is_empty());
                assert_eq!(anchor(&app), Some(3));
            } else {
                assert_ne!(app.selected, Some(3), "{nav:?}");
                assert_eq!(anchor(&app), app.selected);
            }
        }
    }
}

#[test]
fn generation_change_drops_old_range_origin_even_when_index_exists() {
    let mut app = grid(GridViewMode::Thumbnail);
    click(&mut app, 3, false, true);
    assert_eq!(anchor(&app), Some(1));
    app.bump_items_generation();
    click(&mut app, 5, false, true);
    assert_eq!(app.checked, HashSet::from([3, 4, 5]));
    assert_eq!(anchor(&app), Some(3));
    app.move_grid_cursor(Some(2));
    assert_eq!(anchor(&app), Some(2));
}

#[test]
fn filter_redirect_rebases_and_hidden_extension_anchor_uses_cursor() {
    let mut app = grid(GridViewMode::Thumbnail);
    click(&mut app, 3, false, true);
    app.visible_indices = vec![2, 3, 4, 5];
    click(&mut app, 5, false, true);
    assert_eq!(app.checked, HashSet::from([3, 4, 5]));
    assert_eq!(anchor(&app), Some(3));
    app.visible_indices = vec![2, 4];
    app.redirect_selected_to_visible();
    assert_eq!(app.selected, Some(4));
    assert_eq!(anchor(&app), Some(4));
}

#[test]
fn ctrl_deselect_collapse_rebases_to_resulting_cursor() {
    let mut app = grid(GridViewMode::Thumbnail);
    click(&mut app, 3, true, false);
    click(&mut app, 3, true, false);
    assert_eq!(app.selected, Some(1));
    assert_eq!(anchor(&app), Some(1));
    click(&mut app, 4, false, true);
    assert_eq!(app.checked, HashSet::from([1, 2, 3, 4]));
}

#[test]
fn deleting_cursor_rebases_even_when_replacement_has_same_numeric_index() {
    let mut app = grid(GridViewMode::Thumbnail);
    click(&mut app, 3, false, true);
    app.remove_items_batch(&[3]);
    assert_eq!(app.selected, Some(3));
    assert_eq!(anchor(&app), Some(3));
    click(&mut app, 5, false, true);
    assert_eq!(app.checked, HashSet::from([3, 4, 5]));
}

#[cfg(windows)]
#[test]
fn linked_viewer_sync_after_shift_arrow_keeps_grid_extension_origin() {
    let mut app = grid(GridViewMode::Thumbnail);
    let ctx = egui::Context::default();
    app.settings.detached_viewer_enabled = true;
    app.open_fullscreen(1, HistoryTrigger::UserChosen);
    key(&mut app, &ctx, egui::Key::ArrowRight, true);
    assert_eq!(app.selected, Some(2));
    app.sync_detached_viewer_to_selected(&ctx);
    assert_eq!(app.fullscreen_idx, Some(2));
    assert_eq!(anchor(&app), Some(1));
    click(&mut app, 5, false, true);
    assert_eq!(app.checked, HashSet::from([1, 2, 3, 4, 5]));
}

#[cfg(windows)]
#[test]
fn mounted_context_cursor_moves_leave_sibling_anchor_and_selection_unchanged() {
    let mut app = grid(GridViewMode::Thumbnail);
    click(&mut app, 3, false, true);
    let mut sibling = app.split_current_context_preserving_main_grid();
    app.swap_viewer_context_bundle(&mut sibling);
    app.move_grid_cursor(Some(4));
    app.swap_viewer_context_bundle(&mut sibling);
    assert_eq!(app.selected, Some(3));
    assert_eq!(anchor(&app), Some(1));
    assert_eq!(app.checked, HashSet::from([1, 2, 3]));
    app.swap_viewer_context_bundle(&mut sibling);
    assert_eq!(app.selected, Some(4));
    assert_eq!(anchor(&app), Some(4));
}
