//! Exercise the production toolbar's background and child input ownership.
use super::*;
use crate::app::AppTestEnvForTest;
use crate::settings::{ToolbarSectionDisplay as TD, ToolbarSectionId as TS};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};

struct State {
    app: AppTestEnvForTest,
    central: egui::Rect,
    toolbar_passes: usize,
}

fn harness(width: f32) -> Harness<'static, State> {
    let mut app = crate::app::setup_app_for_test();
    for &section in TS::default_order() {
        set_toolbar_section_visible(&mut app.settings, section, false);
    }
    for section in [TS::Sort, TS::Cols, TS::Tags] {
        set_toolbar_section_visible(&mut app.settings, section, true);
    }
    app.settings.toolbar_section_order = vec![TS::Sort, TS::Cols, TS::Tags];
    // Use the real wrapped layout's configured row break at both viewport widths.
    app.settings.toolbar_section_new_row = vec![TS::Cols];
    app.settings.toolbar_sort_display = TD::Buttons;
    app.settings.toolbar_sort_items = crate::settings::SortOrder::all().to_vec();
    app.settings.toolbar_cols_display = TD::Dropdown;
    app.settings.toolbar_cols_items = vec![3, 7];
    app.settings.grid_cols = 3;
    app.settings.toolbar_cols_details_visible = false;
    app.settings.toolbar_tags_display = TD::Buttons;
    app.settings.tags.clear();
    app.settings.show_grid_edge_buttons = false;
    app.settings.show_toolbar_facet_filter = false;
    crate::test_ui::builder()
        .with_size(egui::vec2(width, 400.0))
        .with_context_setup(|ctx| {
            ctx.set_zoom_factor(1.0);
            crate::ui_fonts::configure_fonts(ctx);
            // One step must mean one pass, including the shrink/input boundary.
            ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
        })
        .build_ui_state(
            |root, state: &mut State| {
                crate::test_ui::full_viewport_ui(root, |root| {
                    let ctx = root.ctx().clone();
                    state.app.note_main_chrome_pointer_block_transition(&ctx);
                    state.app.render_toolbar(root);
                    state.toolbar_passes += 1;
                    state.central = egui::CentralPanel::default()
                        .show(root, |_ui| {})
                        .response
                        .rect;
                    state.app.show_toolbar_reset_confirm_dialog(&ctx);
                });
            },
            State {
                app,
                central: egui::Rect::NOTHING,
                toolbar_passes: 0,
            },
        )
}

fn widgets(harness: &Harness<'_, State>) -> Vec<egui::WidgetRect> {
    harness.ctx.viewport(|viewport| {
        viewport
            .prev_pass
            .widgets
            .layers()
            .flat_map(|(_, widgets)| widgets.iter().copied())
            .collect()
    })
}

fn background(harness: &Harness<'_, State>) -> egui::WidgetRect {
    let content = harness.state().app.toolbar_content_rect.unwrap();
    let backgrounds: Vec<_> = widgets(harness)
        .into_iter()
        .filter(|widget| widget.rect == content && widget.sense == egui::Sense::CLICK)
        .collect();
    assert_eq!(backgrounds.len(), 1, "one non-focusable background Ui");
    backgrounds[0]
}

fn child_rects(harness: &Harness<'_, State>) -> Vec<egui::Rect> {
    let all = widgets(harness);
    let content = harness.state().app.toolbar_content_rect.unwrap();
    // Keep leaf controls inside the body, including hover-only labels and disabled
    // buttons. Enclosing panel/frame widgets are not toolbar controls.
    all.iter()
        .filter(|widget| !all.iter().any(|child| child.parent_id == widget.id))
        .filter(|widget| {
            widget.visible && widget.rect != content && content.contains_rect(widget.rect)
        })
        .map(|widget| widget.rect)
        .collect()
}

fn row_ends(harness: &Harness<'_, State>) -> [egui::Pos2; 2] {
    let content = harness.state().app.toolbar_content_rect.unwrap();
    let first = harness
        .get_by_role_and_label(egui::accesskit::Role::Label, "ソート:")
        .rect();
    let last = harness.get_by_role(egui::accesskit::Role::ComboBox).rect();
    assert!(
        last.top() > first.bottom(),
        "toolbar must have multiple rows"
    );
    let children = child_rects(harness);
    let points = [first.center().y, last.center().y].map(|y| {
        let pos = egui::pos2(content.right() - 1.0, y);
        assert!(content.contains(pos));
        assert!(children.iter().all(|rect| !rect.contains(pos)));
        // Both rows must have trailing space, rather than an inter-widget gap.
        assert!(
            children
                .iter()
                .filter(|rect| rect.top() <= y && y <= rect.bottom())
                .all(|rect| rect.right() < pos.x)
        );
        pos
    });
    points
}

fn pointer_button(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn secondary_click(harness: &mut Harness<'_, State>, pos: egui::Pos2) {
    harness.hover_at(pos);
    for pressed in [true, false] {
        harness.event(pointer_button(pos, egui::PointerButton::Secondary, pressed));
        harness.step();
    }
    harness.run();
}

fn assert_no_background_menu(harness: &Harness<'_, State>) {
    assert!(harness.query_by_label("表示するセクション").is_none());
    let bg = background(harness);
    assert!(!egui::Popup::is_id_open(
        &harness.ctx,
        bg.id.with("sticky_context_menu")
    ));
}

#[test]
fn section1876_empty_right_ends_open_visibility_menu_at_both_widths() {
    for width in [1576.0, 2400.0] {
        let mut harness = harness(width);
        harness.run();
        assert_eq!(harness.ctx.zoom_factor(), 1.0);
        let points = row_ends(&harness);
        let rightmost_control = child_rects(&harness)
            .iter()
            .map(|rect| rect.right())
            .reduce(f32::max)
            .expect("toolbar controls");
        // Also cover the panel space beyond the widest row's last control.
        assert!(points[0].x > rightmost_control);
        for pos in points {
            assert!(background(&harness).interact_rect.contains(pos));
            secondary_click(&mut harness, pos);
            assert!(harness.query_by_label("表示するセクション").is_some());
            assert!(harness.query_by_label("既定に戻す").is_none());
            // The popup's own controls retain input and the menu stays open.
            let before = harness.state().app.settings.show_toolbar_rating;
            harness.get_by_label("レーティング (★)").click();
            harness.run();
            assert_eq!(harness.state().app.settings.show_toolbar_rating, !before);
            assert!(harness.query_by_label("表示するセクション").is_some());
            egui::Popup::close_all(&harness.ctx);
            harness.state_mut().app.settings.show_toolbar_rating = false;
            harness.run();
        }
    }
}

#[test]
fn section1876_row_two_children_keep_context_menus_and_combo_clicks() {
    for width in [1576.0, 2400.0] {
        let mut harness = harness(width);
        harness.run();
        let first_y = row_ends(&harness)[0].y;
        for (label, menu_item) in [("列:", "出す列:"), ("タグ:", "表示:")] {
            let child = harness.get_by_role_and_label(egui::accesskit::Role::Label, label);
            assert!(child.rect().top() > first_y);
            child.click_secondary();
            harness.run();
            assert!(harness.query_by_label(menu_item).is_some());
            assert_no_background_menu(&harness);
            egui::Popup::close_all(&harness.ctx);
            harness.run();
        }
        harness.get_by_role(egui::accesskit::Role::ComboBox).click();
        harness.run();
        assert_no_background_menu(&harness);
        harness.get_by_label("7 列").click();
        harness.run();
        assert_eq!(harness.state().app.settings.grid_cols, 7);
        assert_no_background_menu(&harness);
    }
}

#[test]
fn section1876_shrink_retires_old_row_before_the_next_input_pass() {
    for width in [1576.0, 2400.0] {
        let mut harness = harness(width);
        harness.run();
        let old_content = harness.state().app.toolbar_content_rect.unwrap();
        let pos = row_ends(&harness)[1];
        let passes = harness.state().toolbar_passes;
        harness.state_mut().app.settings.show_toolbar_cols = false;
        harness.state_mut().app.settings.show_toolbar_tags = false;
        harness.step(); // Exactly one pass draws the smaller toolbar.
        assert_eq!(harness.state().toolbar_passes, passes + 1);
        let content = harness.state().app.toolbar_content_rect.unwrap();
        assert!(content.height() < old_content.height());
        assert!(old_content.contains(pos) && pos.y > content.bottom());
        assert!(harness.state().central.contains(pos));
        // No hover/settling pass: deliver the complete click in the next input pass.
        harness.input_mut().events.extend([
            egui::Event::PointerMoved(pos),
            pointer_button(pos, egui::PointerButton::Secondary, true),
            pointer_button(pos, egui::PointerButton::Secondary, false),
        ]);
        harness.step();
        assert_eq!(harness.state().toolbar_passes, passes + 2);
        assert_no_background_menu(&harness);
        harness.run();
        assert_no_background_menu(&harness);
    }
}

#[test]
fn section1876_background_accesskit_bounds_are_valid_and_children_remain() {
    let mut harness = harness(1576.0);
    harness.run();
    let bg = background(&harness);
    let root = harness.queryable_node();
    let node = root
        .children_recursive()
        .find(|node| node.accesskit_node().locate().0 == bg.id.accesskit_id())
        .expect("background Ui remains in the accessibility tree");
    if let Some(bounds) = node.accesskit_node().data().bounds() {
        assert!(
            [bounds.x0, bounds.y0, bounds.x1, bounds.y1]
                .into_iter()
                .all(f64::is_finite)
        );
        assert!(bounds.x0 <= bounds.x1 && bounds.y0 <= bounds.y1);
    }
    assert!(
        node.query_by_role_and_label(egui::accesskit::Role::Label, "ソート:")
            .is_some()
    );
    assert!(
        node.query_by_role_and_label(egui::accesskit::Role::Label, "タグ:")
            .is_some()
    );
    assert!(
        node.query_by_role(egui::accesskit::Role::ComboBox)
            .is_some()
    );
}

#[test]
fn section1876_modal_blocks_row_two_background_and_releases_it_on_close() {
    let mut harness = harness(2400.0);
    harness.run();
    let pos = row_ends(&harness)[1];
    harness.state_mut().app.show_toolbar_reset_confirm = true;
    harness.run();
    assert!(harness.state().app.common_modal_dialog_open());
    assert!(harness.query_by_label("キャンセル").is_some());
    assert!(!background(&harness).enabled);
    secondary_click(&mut harness, pos);
    assert_no_background_menu(&harness);
    harness.get_by_label("キャンセル").click();
    harness.run();
    assert!(!harness.state().app.common_modal_dialog_open());
    secondary_click(&mut harness, pos);
    assert!(harness.query_by_label("表示するセクション").is_some());
}

#[test]
fn section1876_long_touch_on_row_two_empty_space_opens_visibility_menu() {
    let mut harness = harness(1576.0);
    harness.run();
    let pos = row_ends(&harness)[1];
    let bg = background(&harness);
    let touch = |phase| egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(1),
        phase,
        pos,
        force: None,
    };
    harness.input_mut().events.extend([
        touch(egui::TouchPhase::Start),
        egui::Event::PointerMoved(pos),
        pointer_button(pos, egui::PointerButton::Primary, true),
    ]);
    harness.step();
    assert_no_background_menu(&harness);
    let duration = harness
        .ctx
        .options(|options| options.input_options.max_click_duration);
    let held_until = harness.ctx.input(|input| input.time + duration + 0.1);
    harness.input_mut().time = Some(held_until);
    harness.step();
    assert!(harness.ctx.read_response(bg.id).unwrap().long_touched());
    harness.input_mut().time = None;
    harness.run();
    assert!(harness.query_by_label("表示するセクション").is_some());
    harness.input_mut().events.extend([
        touch(egui::TouchPhase::End),
        pointer_button(pos, egui::PointerButton::Primary, false),
        egui::Event::PointerGone,
    ]);
    harness.step();
}

#[test]
fn section1876_row_two_heading_drag_reorders_sections() {
    let mut harness = harness(1576.0);
    harness
        .state_mut()
        .app
        .settings
        .toolbar_section_drag_enabled = true;
    harness.run();
    let start = harness
        .get_by_role_and_label(egui::accesskit::Role::Label, "列:")
        .rect()
        .left_center()
        + egui::vec2(2.0, 0.0);
    let end = harness
        .get_by_role_and_label(egui::accesskit::Role::Label, "ソート:")
        .rect()
        .left_center();
    assert!(start.y > end.y);
    for event in [
        egui::Event::PointerMoved(start),
        pointer_button(start, egui::PointerButton::Primary, true),
        egui::Event::PointerMoved(end),
        pointer_button(end, egui::PointerButton::Primary, false),
    ] {
        harness.event(event);
        harness.step();
    }
    harness.run();
    let order = &harness.state().app.settings.toolbar_section_order;
    let index = |section| order.iter().position(|&id| id == section).unwrap();
    assert!(index(TS::Cols) < index(TS::Sort));
    assert_no_background_menu(&harness);
    assert!(
        !harness
            .ctx
            .plugin::<egui::text_selection::LabelSelectionState>()
            .lock()
            .has_selection()
    );
}

#[test]
fn section1876_row_two_address_context_menu_selects_text() {
    let mut harness = harness(1576.0);
    let text = "C:\\section1876\\address selection";
    {
        let app = &mut harness.state_mut().app;
        app.settings.show_toolbar_cols = false;
        app.settings.show_toolbar_tags = false;
        app.settings.show_toolbar_folder = true;
        app.settings.toolbar_section_order = vec![TS::Sort, TS::Folder];
        app.settings.toolbar_section_new_row = vec![TS::Folder];
        app.address = text.to_string();
    }
    harness.run();
    let field = harness.get_by_role(egui::accesskit::Role::TextInput);
    let first = harness
        .get_by_role_and_label(egui::accesskit::Role::Label, "ソート:")
        .rect();
    assert!(field.rect().top() > first.bottom());
    field.click_secondary();
    harness.run();
    assert_no_background_menu(&harness);
    harness.get_by_label("すべて選択").click();
    harness.run();
    let id = egui::Id::unique("folder_address_input");
    let state = egui::TextEdit::load_state(&harness.ctx, id).unwrap();
    assert_eq!(
        state.cursor.char_range().unwrap(),
        egui::text::CCursorRange::select_all_str(text)
    );
    assert!(harness.ctx.memory(|memory| memory.has_focus(id)));
    assert_eq!(harness.state().app.address, text);
    assert_no_background_menu(&harness);
}

#[test]
fn section1876_row_two_pinned_tag_secondary_click_keeps_tag_action() {
    let mut harness = harness(1576.0);
    let path;
    {
        let app = &mut harness.state_mut().app;
        let folder = app.tmp.path().join("section1876-tags");
        std::fs::create_dir_all(&folder).unwrap();
        path = folder.join("image.jpg");
        std::fs::write(&path, b"fixture").unwrap();
        app.settings.auto_fullscreen_image_folders = false;
        app.load_folder_and_finish_for_test(folder);
        app.finish_page_identity_preparation_for_test();
        app.selected = Some(0);
        app.last_main_focused = true;
        app.folder_pane.set_focus_grid();
        app.settings.tags = vec![crate::settings::TagDef::new("section1876".into())];
        assert_eq!(
            app.tag_target_paths(crate::app::ActionSurface::MainWindow),
            [path.clone()]
        );
    }
    harness.run();
    let tag = harness.get_by_label("#section1876");
    let first = harness
        .get_by_role_and_label(egui::accesskit::Role::Label, "ソート:")
        .rect();
    assert!(tag.rect().top() > first.bottom());
    tag.click_secondary();
    harness.run();
    assert_eq!(
        harness
            .state()
            .app
            .tags_cache
            .get(&crate::tags_db::item_key_for_path(&path)),
        Some(&vec!["#section1876".to_string()])
    );
    assert_no_background_menu(&harness);
}
