//! Handler-level coverage: drive egui's real pointer responses through the grid dispatcher.

use super::*;
use crate::app::{AppTestEnvForTest, GridScrollGeometry, setup_app_for_test};
use crate::folder_order::{FolderOrderMode, FolderOrderRecord, physical_item_key};
use crate::settings::{GridClickSelectionMode, GridViewMode, SortOrder};
use egui_kittest::Harness;
use std::path::PathBuf;
use std::time::Duration;

const CELL_W: f32 = 120.0;
const ROW_H: f32 = 100.0;

struct DragHandlerState {
    app: AppTestEnvForTest,
    folder: PathBuf,
    rects: Vec<(usize, egui::Rect)>,
    clip: egui::Rect,
    active: bool,
    touch_derived: bool,
    replay_finish: bool,
}

fn fixture() -> DragHandlerState {
    let mut app = setup_app_for_test();
    app.settings.sidecar_backup_enabled = false;
    app.settings.tag_sidecar_backup_enabled = false;
    app.settings.auto_fullscreen_image_folders = false;
    app.settings.grid_click_selection_mode = GridClickSelectionMode::Explorer;
    app.settings.grid_open_selected_item_on_click = false;
    app.settings.sort_order = SortOrder::FileName;
    app.settings.grid_view_mode = GridViewMode::Thumbnail;
    app.active_quick_folder_slot = None;
    let folder = app.tmp.path().join("folder-order-pointer");
    std::fs::create_dir_all(&folder).unwrap();
    for name in ["a-folder", "b-folder"] {
        std::fs::create_dir(folder.join(name)).unwrap();
    }
    for name in ["c.jpg", "d.jpg", "e.jpg", "f.jpg", "hidden.txt"] {
        std::fs::write(folder.join(name), b"fixture").unwrap();
    }
    app.load_folder(folder.clone());
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.items.len(), 6);
    assert!(matches!(app.items[0], GridItem::Folder(_)));
    assert!(matches!(app.items[2], GridItem::Image(_)));
    app.folder_order.enter_manual();
    assert!(app.folder_manual_order_drag_enabled());
    DragHandlerState {
        app,
        folder,
        rects: Vec::new(),
        clip: egui::Rect::NOTHING,
        active: false,
        touch_derived: false,
        replay_finish: false,
    }
}

fn harness(state: DragHandlerState) -> Harness<'static, DragHandlerState> {
    crate::test_ui::builder()
        .with_size(egui::vec2(360.0, 380.0))
        .with_step_dt(0.01)
        .build_ui_state(
            |root_ui, state| {
                let ctx = root_ui.ctx().clone();
                let ending = state.app.take_folder_order_drag_end(&ctx);
                state.app.begin_grid_click_input_frame(&ctx);
                egui::CentralPanel::default().show(root_ui, |ui| {
                    let origin = ui.max_rect().min + egui::vec2(12.0, 12.0);
                    state.clip =
                        egui::Rect::from_min_size(origin, egui::vec2(CELL_W * 2.0, ROW_H * 3.0));
                    let geometry = GridScrollGeometry::new(
                        2,
                        ROW_H,
                        ROW_H * 3.0,
                        state.app.visible_indices.len().div_ceil(2) as f32 * ROW_H,
                    );
                    state.app.grid_scroll.record_geometry(geometry);
                    let suppress = state.app.begin_grid_right_drag_primary_frame(&ctx);
                    let previous = state.app.begin_grid_primary_click_frame(
                        &ctx,
                        !suppress && ctx.input(|input| input.pointer.primary_clicked()),
                    );
                    let touch = crate::touch_correlation::drive_egui_grid_touch_input(
                        &ctx,
                        crate::touch_input::TapZoneGeometry {
                            surface: state.clip,
                            excluded: Vec::new(),
                            behavior: crate::touch_input::TouchSurfaceBehavior::Grid,
                        },
                        state.app.grid_scroll_scope(),
                        true,
                    );
                    state.rects.clear();
                    for (slot, index) in state.app.visible_indices.clone().into_iter().enumerate() {
                        let rect = egui::Rect::from_min_size(
                            origin
                                + egui::vec2((slot % 2) as f32 * CELL_W, (slot / 2) as f32 * ROW_H),
                            egui::vec2(CELL_W, ROW_H),
                        );
                        state.rects.push((index, rect));
                        state.app.handle_cell_interaction(
                            ui,
                            &ctx,
                            rect,
                            index,
                            &crate::thumb_overlay_layout::ThumbnailOverlayLayout::default(),
                            state.touch_derived || touch.has_touch_derived_pointer_activity(),
                            suppress,
                            &touch,
                            previous,
                            None,
                        );
                    }
                    let pointer = ctx.input(|input| input.pointer.interact_pos());
                    let hit_cell = pointer
                        .is_some_and(|pos| state.rects.iter().any(|(_, rect)| rect.contains(pos)));
                    state.app.handle_grid_background_primary_click(
                        &ctx, state.clip, hit_cell, hit_cell, suppress, previous, &touch,
                    );
                    state.app.finish_folder_order_drag_frame(
                        ui, state.clip, origin, geometry, CELL_W, ending,
                    );
                    if state.replay_finish {
                        let after_first = state.app.grid_scroll.offset();
                        state.app.finish_folder_order_drag_frame(
                            ui, state.clip, origin, geometry, CELL_W, None,
                        );
                        assert_eq!(
                            state.app.grid_scroll.offset(),
                            after_first,
                            "a repeated pass cannot accrue a second scroll step in the same frame"
                        );
                    }
                    state.active = state.app.folder_order_drag_active(&ctx);
                });
            },
            state,
        )
}

fn rect(h: &Harness<'_, DragHandlerState>, index: usize) -> egui::Rect {
    h.state().rects.iter().find(|(i, _)| *i == index).unwrap().1
}

fn button(
    h: &mut Harness<'_, DragHandlerState>,
    pos: egui::Pos2,
    button: egui::PointerButton,
    pressed: bool,
) {
    h.hover_at(pos);
    h.event(egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
}

fn start(h: &mut Harness<'_, DragHandlerState>, index: usize) {
    let pos = rect(h, index).center();
    button(h, pos, egui::PointerButton::Primary, true);
    h.hover_at(pos + egui::vec2(20.0, 0.0));
    h.step();
}

fn drop_at(h: &mut Harness<'_, DragHandlerState>, pos: egui::Pos2) {
    h.hover_at(pos);
    h.step();
    button(h, pos, egui::PointerButton::Primary, false);
}

fn keys(h: &Harness<'_, DragHandlerState>) -> Vec<String> {
    h.state()
        .app
        .items
        .iter()
        .filter_map(physical_item_key)
        .collect()
}

fn read_record(h: &Harness<'_, DragHandlerState>) -> Option<FolderOrderRecord> {
    h.state()
        .app
        .folder_order_handle()
        .unwrap()
        .read(&h.state().folder)
        .unwrap()
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap()
}

#[test]
fn folder_order_drag_handler_manual_thumbnail_starts_without_native_payload() {
    let mut h = harness(fixture());
    start(&mut h, 0);
    assert!(h.state().active);
    assert!(h.state().app.pending_native_drag.is_none());
    assert_eq!(
        h.state().app.folder_order.mode,
        FolderOrderMode::ManualUnrecorded
    );
    assert!(read_record(&h).is_none());
}

#[test]
fn folder_order_drag_handler_autoscroll_uses_external_rows_once_per_frame() {
    let mut state = fixture();
    for n in 0..12 {
        std::fs::write(state.folder.join(format!("z{n:02}.jpg")), b"fixture").unwrap();
    }
    state.app.load_folder(state.folder.clone());
    state.app.finish_page_identity_preparation_for_test();
    state.app.folder_order.enter_manual();
    state.replay_finish = true;
    let mut h = harness(state);
    start(&mut h, 0);
    assert!(h.state().active);
    let edge = h.state().clip.left_bottom() + egui::vec2(10.0, -1.0);
    h.hover_at(edge);
    for _ in 0..10 {
        h.step();
        assert_eq!(h.state().app.grid_scroll.offset() % ROW_H, 0.0);
    }
    let offset = h.state().app.grid_scroll.offset();
    assert!(
        offset > 0.0,
        "fractional edge movement must eventually advance a row"
    );
    let geometry = h.state().app.grid_scroll.geometry();
    assert_eq!(
        h.state_mut()
            .app
            .grid_scroll
            .resolve_layout(geometry, false, None, true)
            .position,
        egui::scroll_area::ExternalScrollPosition::Set(offset)
    );
    assert!(read_record(&h).is_none());
}

#[test]
fn folder_order_drag_handler_normal_details_stack_and_ineligible_preserve_native_payload() {
    for route in 0..4 {
        let mut state = fixture();
        match route {
            0 => {
                state.app.folder_order.leave_manual();
            }
            1 => state.app.settings.grid_view_mode = GridViewMode::Details,
            2 => state.app.stack_mode_requested = true,
            3 => state.app.items_are_subfolder_expansion_view = true,
            _ => unreachable!(),
        }
        assert!(!state.app.folder_manual_order_drag_enabled());
        let mut h = harness(state);
        let path = h.state().app.items[0]
            .drag_source_path()
            .unwrap()
            .to_path_buf();
        start(&mut h, 0);
        assert!(!h.state().active, "route {route}");
        assert_eq!(
            h.state().app.pending_native_drag.as_ref().unwrap().paths,
            vec![path]
        );
        assert!(read_record(&h).is_none());
    }
}

#[test]
fn folder_order_drag_handler_touch_and_recent_native_drag_do_not_start() {
    for touch in [false, true] {
        let mut state = fixture();
        state.touch_derived = touch;
        state.app.native_drag_just_finished = !touch;
        let mut h = harness(state);
        start(&mut h, 0);
        assert!(!h.state().active);
        assert!(h.state().app.pending_native_drag.is_none());
        assert!(read_record(&h).is_none());
    }
}

#[test]
fn folder_order_drag_handler_raw_touch_with_primary_mirror_does_not_start() {
    let mut h = harness(fixture());
    let start = rect(&h, 0).center();
    h.hover_at(start);
    h.event(egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(1),
        phase: egui::TouchPhase::Start,
        pos: start,
        force: None,
    });
    button(&mut h, start, egui::PointerButton::Primary, true);
    let moved = start + egui::vec2(30.0, 0.0);
    h.event(egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(1),
        phase: egui::TouchPhase::Move,
        pos: moved,
        force: None,
    });
    h.hover_at(moved);
    h.step();
    assert!(!h.state().active);
    assert!(h.state().app.pending_native_drag.is_none());
    assert!(read_record(&h).is_none());
}

#[test]
fn folder_order_drag_handler_secondary_drag_does_not_start() {
    let mut h = harness(fixture());
    let pos = rect(&h, 0).center();
    button(&mut h, pos, egui::PointerButton::Secondary, true);
    h.hover_at(pos + egui::vec2(25.0, 0.0));
    h.step();
    assert!(!h.state().active);
    assert!(h.state().app.pending_native_drag.is_none());
    assert!(read_record(&h).is_none());
}

#[test]
fn folder_order_drag_handler_right_drag_primary_suppression_does_not_start() {
    let mut h = harness(fixture());
    let pos = rect(&h, 0).center();
    button(&mut h, pos, egui::PointerButton::Primary, true);
    h.state_mut().app.grid_right_drag_primary_suppression =
        GridRightDragPrimarySuppression::UntilRelease;
    h.hover_at(pos + egui::vec2(25.0, 0.0));
    h.step();
    assert!(!h.state().active);
    assert!(h.state().app.pending_native_drag.is_none());
    assert!(read_record(&h).is_none());
}

#[test]
fn folder_order_drag_handler_cross_row_drop_moves_visible_checked_group_in_relative_order() {
    let mut state = fixture();
    state.app.checked.extend([0, 2, 4]);
    state.app.visible_indices.retain(|index| *index != 4);
    let mut h = harness(state);
    let before = keys(&h);
    start(&mut h, 0);
    assert!(h.state().active);
    let target = rect(&h, 5).right_center() - egui::vec2(8.0, 0.0);
    drop_at(&mut h, target);
    assert!(!h.state().active);
    assert!(h.state().app.pending_native_drag.is_none());
    let record = read_record(&h).expect("accepted drop must enqueue a persistent save");
    assert_eq!(
        record.items,
        [1, 3, 5, 0, 4, 2].map(|index| before[index].clone())
    );
    assert!(record.active);
    assert_eq!(
        h.state().app.folder_order.mode,
        FolderOrderMode::ManualRecorded
    );
    // Existing refresh owns replacement: no local items permutation is allowed.
    assert_eq!(keys(&h), before);
}

#[test]
fn folder_order_drag_handler_unchecked_source_moves_only_grabbed_item() {
    let mut state = fixture();
    state.app.checked.extend([2, 4]);
    let mut h = harness(state);
    let before = keys(&h);
    start(&mut h, 0);
    assert!(h.state().active);
    let target = rect(&h, 5).right_center() - egui::vec2(8.0, 0.0);
    drop_at(&mut h, target);
    let record = read_record(&h).unwrap();
    assert_eq!(
        record.items,
        [1, 2, 3, 4, 5, 0].map(|index| before[index].clone())
    );
}

#[test]
fn folder_order_drag_handler_recorded_inventory_only_child_keeps_its_saved_slot() {
    let mut state = fixture();
    let before = state
        .app
        .items
        .iter()
        .filter_map(physical_item_key)
        .collect::<Vec<_>>();
    let hidden = crate::folder_order::folder_key(&state.folder.join("hidden.txt"));
    let mut recorded = before.clone();
    recorded.insert(2, hidden.clone());
    let record = FolderOrderRecord {
        active: true,
        revision: 1,
        items: recorded.clone(),
    };
    state.app.folder_order.mode = FolderOrderMode::ManualRecorded;
    state.app.folder_order.record = Some(record.clone());
    state.app.folder_order.input =
        crate::folder_order::FolderOrderListingInput::from_read(Ok(Some(record)));
    state
        .app
        .folder_order_handle()
        .unwrap()
        .save(
            &state.folder,
            crate::folder_order::FolderOrderSave {
                listing_order: recorded,
                inventory: state.app.folder_order.inventory.clone(),
                revision: state.app.folder_order.order_revision,
            },
        )
        .unwrap();
    let mut h = harness(state);
    assert!(read_record(&h).is_some());
    start(&mut h, 0);
    let target = rect(&h, 5).right_center() - egui::vec2(8.0, 0.0);
    drop_at(&mut h, target);
    let mut expected = [1, 2, 3, 4, 5, 0]
        .map(|index| before[index].clone())
        .to_vec();
    expected.insert(2, hidden);
    assert_eq!(read_record(&h).unwrap().items, expected);
    assert_eq!(keys(&h), before);
}

#[test]
fn folder_order_drag_handler_positionless_release_cancels_without_using_latest_hover() {
    let mut h = harness(fixture());
    start(&mut h, 0);
    assert!(h.state().active);
    let target = rect(&h, 5).right_center() - egui::vec2(8.0, 0.0);
    h.hover_at(target);
    h.step();
    h.event(egui::Event::PointerButtonReleased {
        button: egui::PointerButton::Primary,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    assert!(!h.state().active);
    assert!(read_record(&h).is_none());
}

#[test]
fn folder_order_drag_handler_left_half_inserts_before_target_with_frozen_selection() {
    let mut state = fixture();
    state.app.checked.extend([3, 5]);
    let mut h = harness(state);
    let before = keys(&h);
    start(&mut h, 3);
    assert!(h.state().active);
    // Later selection changes must not change the source group captured on start.
    h.state_mut().app.checked.clear();
    h.state_mut().app.checked.insert(1);
    let target = rect(&h, 0).left_center() + egui::vec2(8.0, 0.0);
    drop_at(&mut h, target);
    let record = read_record(&h).unwrap();
    assert_eq!(
        record.items,
        [3, 5, 0, 1, 2, 4].map(|index| before[index].clone())
    );
}

#[test]
fn folder_order_drag_handler_same_position_and_outside_drop_do_not_write() {
    for outside in [false, true] {
        let mut h = harness(fixture());
        let revision = h.state().app.folder_order.order_revision;
        let before = keys(&h);
        start(&mut h, 0);
        assert!(h.state().active);
        let target = if outside {
            h.state().clip.right_top() + egui::vec2(30.0, 20.0)
        } else {
            rect(&h, 0).left_center() + egui::vec2(8.0, 0.0)
        };
        drop_at(&mut h, target);
        assert!(!h.state().active);
        assert!(read_record(&h).is_none());
        assert_eq!(h.state().app.folder_order.order_revision, revision);
        assert_eq!(
            h.state().app.folder_order.mode,
            FolderOrderMode::ManualUnrecorded
        );
        assert_eq!(keys(&h), before);
    }
}

fn primary_edge(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn folder_order_drag_handler_old_positioned_release_commits_before_fresh_drag_same_batch() {
    let mut h = harness(fixture());
    let before = keys(&h);
    start(&mut h, 0);
    assert!(h.state().active);
    let revision = h.state().app.folder_order.order_revision;
    let old_target = rect(&h, 5).right_center() - egui::vec2(8.0, 0.0);
    let fresh_start = rect(&h, 2).center();
    // The terminal belongs to the old captured group, even when selection and
    // a new press already refer to a different source in the same egui pass.
    h.state_mut().app.checked.clear();
    h.state_mut().app.checked.insert(2);
    h.event(primary_edge(old_target, false));
    h.event(primary_edge(fresh_start, true));
    h.event(egui::Event::PointerMoved(
        fresh_start + egui::vec2(20.0, 0.0),
    ));
    h.step();
    assert!(h.state().app.folder_order.order_revision > revision);
    assert!(
        !h.state().active,
        "the committed order retires any old-revision gesture"
    );
    assert!(h.state().app.pending_native_drag.is_none());
    let record = read_record(&h).unwrap();
    assert_eq!(
        record.items,
        [1, 2, 3, 4, 5, 0].map(|index| before[index].clone())
    );
    // A pending order refresh must not let the fresh hold write the old listing.
    h.step();
    assert!(!h.state().active);
    assert_eq!(read_record(&h).unwrap(), record);
}

#[test]
fn folder_order_drag_handler_old_noop_release_preserves_fresh_drag_same_batch() {
    let mut h = harness(fixture());
    let before = keys(&h);
    start(&mut h, 0);
    assert!(h.state().active);
    let revision = h.state().app.folder_order.order_revision;
    let old_target = rect(&h, 0).left_center() + egui::vec2(8.0, 0.0);
    let fresh_start = rect(&h, 2).center();
    h.event(primary_edge(old_target, false));
    h.event(primary_edge(fresh_start, true));
    h.event(egui::Event::PointerMoved(
        fresh_start + egui::vec2(20.0, 0.0),
    ));
    h.step();
    assert!(read_record(&h).is_none());
    assert_eq!(h.state().app.folder_order.order_revision, revision);
    // As in the book/collection handlers, click_and_drag matures a new press
    // on the next pass. The old release must not end that fresh gesture.
    h.step();
    assert!(h.state().active);
    assert!(h.state().app.pending_native_drag.is_none());
    let target = rect(&h, 5).right_center() - egui::vec2(8.0, 0.0);
    drop_at(&mut h, target);
    assert_eq!(
        read_record(&h).unwrap().items,
        [0, 1, 3, 4, 5, 2].map(|index| before[index].clone())
    );
}

#[test]
fn folder_order_drag_handler_old_positionless_release_preserves_fresh_drag_same_batch() {
    let mut h = harness(fixture());
    let before = keys(&h);
    start(&mut h, 0);
    assert!(h.state().active);
    let revision = h.state().app.folder_order.order_revision;
    let fresh_start = rect(&h, 2).center();
    h.event(egui::Event::PointerGone);
    h.event(egui::Event::PointerButtonReleased {
        button: egui::PointerButton::Primary,
        modifiers: egui::Modifiers::NONE,
    });
    h.event(primary_edge(fresh_start, true));
    h.event(egui::Event::PointerMoved(
        fresh_start + egui::vec2(20.0, 0.0),
    ));
    h.step();
    assert!(read_record(&h).is_none());
    assert_eq!(h.state().app.folder_order.order_revision, revision);
    h.step();
    assert!(h.state().active);
    assert!(h.state().app.pending_native_drag.is_none());
    let target = rect(&h, 5).right_center() - egui::vec2(8.0, 0.0);
    drop_at(&mut h, target);
    assert_eq!(
        read_record(&h).unwrap().items,
        [0, 1, 3, 4, 5, 2].map(|index| before[index].clone())
    );
}

#[test]
fn folder_order_drag_handler_owner_changes_cancel_before_drop_without_write() {
    for change in 0..10 {
        let mut h = harness(fixture());
        start(&mut h, 0);
        assert!(h.state().active);
        let target = rect(&h, 5).right_center() - egui::vec2(8.0, 0.0);
        match change {
            0 => {
                let other = h.state().folder.join("other");
                h.state_mut().app.current_folder = Some(other);
            }
            1 => h.state_mut().app.items_generation += 1,
            2 => h.state_mut().app.folder_order.order_revision += 1,
            3 => h.state_mut().app.settings.grid_view_mode = GridViewMode::Details,
            4 => {
                h.state_mut().app.visible_indices.pop();
            }
            5 => h.state_mut().app.color_filter.enabled = true,
            6 => h.state_mut().app.settings.rating_filter[0] = false,
            7 => h.state_mut().app.grid_scroll.renew_session(),
            8 => {
                h.state_mut().app.settings.facet_filter.name_query =
                    "changed without rebuilding".into()
            }
            9 => {
                let hidden = h.state().app.settings.show_hidden_files;
                h.state_mut().app.settings.show_hidden_files = !hidden;
            }
            _ => unreachable!(),
        }
        h.step();
        assert!(!h.state().active, "change {change}");
        drop_at(&mut h, target);
        assert!(h.state().app.pending_native_drag.is_none());
        assert!(read_record(&h).is_none(), "change {change}");
        assert_eq!(
            h.state().app.folder_order.mode,
            FolderOrderMode::ManualUnrecorded
        );
    }
}
