//! §1.855: exercise the actual chrome, key handler and grid reveal path.
use super::*;
use crate::settings::{DetailsSortKey, GridViewMode, SortOrder, ToolbarSectionDisplay};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};

const VISIBILITY: &str = "先頭・末尾ボタン (ソートの隣)";
const FACET_PLACEMENT_NOTE: &str = "いまは絞り込みバーのソートの隣に表示しています";

fn harness(facet: bool, view: GridViewMode) -> Harness<'static, phase_c_support::AppTestEnv> {
    let mut env = phase_c_support::setup_app();
    let folder = env.tmp.path().join("grid-edge-buttons");
    for name in ["a-folder", "z-folder"] {
        std::fs::create_dir_all(folder.join(name)).unwrap();
    }
    for index in 0..48 {
        std::fs::write(folder.join(format!("image-{index:02}.jpg")), b"image").unwrap();
    }
    env.settings.auto_fullscreen_image_folders = false;
    env.settings.sort_order = SortOrder::FileName;
    env.settings.grid_display_order = Default::default(); // folders before images
    env.load_folder_with_scan(folder, None);
    env.finish_page_identity_preparation_for_test();
    env.settings.grid_view_mode = view;
    env.settings.grid_cols = 4;
    env.settings.details_sort_key = DetailsSortKey::Toolbar;
    env.settings.details_show_page_count = false;
    env.settings.details_show_created = false;
    env.settings.details_show_image_dimensions = false;
    env.settings.details_show_video_duration = false;
    env.settings.details_show_video_dimensions = false;
    env.settings.details_show_video_codec = false;
    env.settings.selection_info_display_mode = crate::settings::SelectionInfoDisplayMode::Hidden;
    env.settings.show_toolbar_folder_tree_button = false;
    env.settings.show_toolbar_effetune = false;
    env.settings.show_toolbar_bookshelf = false;
    env.settings.show_toolbar_collections = false;
    env.settings.show_toolbar_cols = false;
    env.settings.show_toolbar_aspect = false;
    env.settings.show_toolbar_rating = false;
    env.settings.show_toolbar_favorites = false;
    env.settings.show_toolbar_smart_folders = false;
    env.settings.show_toolbar_tags = false;
    env.settings.show_toolbar_folder = false;
    env.settings.show_toolbar_search_container = false;
    env.settings.show_toolbar_search_local = false;
    env.settings.show_toolbar_search_metadata = false;
    env.settings.show_toolbar_search_everything = false;
    env.settings.show_toolbar_sort = true;
    env.settings.show_toolbar_facet_filter = facet;
    env.settings.show_facet_sort = true;
    env.settings.show_grid_edge_buttons = true;
    env.settings.toolbar_sort_display = ToolbarSectionDisplay::Dropdown;
    env.settings.toolbar_sort_items = vec![SortOrder::FileName, SortOrder::FileNameDesc];
    env.settings.toolbar_facet_filter_items.clear();
    env.rebuild_details_order();
    env.last_main_focused = true;
    env.folder_pane.set_focus_grid();
    env.thumbnails = vec![ThumbnailState::Failed; env.items.len()];

    crate::test_ui::builder()
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .with_size(egui::vec2(900.0, 420.0))
        .build_ui_state(
            |root_ui, env: &mut phase_c_support::AppTestEnv| {
                let ctx = root_ui.ctx().clone();
                // Match production hydration input retirement (backlog §1.854).
                env.edit_preview_repaint_ctx = Some(ctx.clone());
                env.note_main_chrome_pointer_block_transition(&ctx);
                env.keyboard_owner_for_pass(&ctx);
                let _ = env.handle_keyboard(&ctx);
                env.render_toolbar(root_ui);
                env.render_facet_filter_bar(root_ui);
                let _ = env.render_grid(root_ui);
                env.show_toolbar_reset_confirm_dialog(&ctx);
            },
            env,
        )
}

fn assert_revealed(harness: &Harness<'_, phase_c_support::AppTestEnv>) {
    let env = harness.state();
    let position = env
        .current_grid_order()
        .iter()
        .position(|&index| Some(index) == env.selected)
        .unwrap();
    let geometry = env.grid_scroll.geometry();
    let top = (position / geometry.cols) as f32 * geometry.row_h;
    let offset = env.grid_scroll.offset();
    assert!(top >= offset - 1.0, "row above viewport: {top} / {offset}");
    assert!(top < offset + geometry.viewport_h, "row below viewport");
    assert!(
        !env.grid_scroll.reveal_requested(),
        "real grid must consume the reveal"
    );
    // Both renderers publish this real ScrollArea response. The separate
    // grid_scroll_render_witness is recorded only by the thumbnail renderer.
    let viewport = harness
        .ctx
        .read_response(App::grid_scroll_transport_id(&harness.ctx).with("area"))
        .expect("scrollable grid body must have a drag response")
        .interact_rect;
    assert!(
        env.selected_cell_rect
            .unwrap()
            .intersect(viewport)
            .is_positive()
    );
}

fn check_buttons_against_keys(harness: &mut Harness<'_, phase_c_support::AppTestEnv>) {
    for (label, key, last) in [
        ("末尾", egui::Key::End, true),
        ("先頭", egui::Key::Home, false),
    ] {
        assert_eq!(
            harness.get_all_by_label(label).count(),
            1,
            "one placement only"
        );
        let middle = harness.state().current_grid_order()[20];
        {
            let env = harness.state_mut();
            env.selected = Some(middle);
            let offset = if last {
                0.0
            } else {
                env.grid_scroll.geometry().max_offset
            };
            env.grid_scroll.set_position(offset);
        }
        harness.run();
        harness.ctx.memory_mut(|memory| {
            if let Some(id) = memory.focused() {
                memory.surrender_focus(id);
            }
        });
        harness.key_press(key);
        harness.run();
        let expected = harness.state().selected;
        assert_revealed(harness);
        let key_offset = harness.state().grid_scroll.offset();
        {
            let env = harness.state_mut();
            env.selected = Some(middle);
            let offset = if last {
                0.0
            } else {
                env.grid_scroll.geometry().max_offset
            };
            env.grid_scroll.set_position(offset);
        }
        harness.run();
        harness.get_by_label(label).click();
        harness.run();
        assert_eq!(
            harness.state().selected,
            expected,
            "{label} must match {key:?}"
        );
        assert_revealed(harness);
        assert_eq!(harness.state().grid_scroll.offset(), key_offset);
    }
}

#[test]
fn section1855_buttons_match_home_end_and_reveal_after_sort_on_both_surfaces() {
    for facet in [false, true] {
        for view in [GridViewMode::Thumbnail, GridViewMode::Details] {
            let mut harness = harness(facet, view);
            harness.run();
            assert!(matches!(
                harness.state().items[harness.state().current_grid_order()[0]],
                GridItem::Folder(_)
            ));
            assert!(matches!(
                harness.state().items[*harness.state().current_grid_order().last().unwrap()],
                GridItem::Image(_)
            ));
            check_buttons_against_keys(&mut harness);
            let old_first = harness.state().items[0].name().into_owned();
            // Change order through the real dropdown, then adopt its real worker reply.
            harness
                .get_all_by_role(egui::accesskit::Role::ComboBox)
                .into_iter()
                .find(|node| node.value().as_deref() == Some(SortOrder::FileName.short_label()))
                .unwrap()
                .click();
            harness.run();
            harness
                .get_by_label(SortOrder::FileNameDesc.short_label())
                .click();
            harness.run();
            let ready = mutation_refresh_wait_for_order_scan(harness.state_mut());
            mutation_refresh_resolve_order_scan(harness.state_mut(), ready);
            harness
                .state_mut()
                .finish_page_identity_preparation_for_test();
            let count = harness.state().items.len();
            harness.state_mut().thumbnails = vec![ThumbnailState::Failed; count];
            harness.run();
            assert_ne!(harness.state().items[0].name(), old_first);
            check_buttons_against_keys(&mut harness);
            if view == GridViewMode::Details {
                // A header-owned order must use details_order even though Sort is locked.
                harness
                    .state_mut()
                    .set_details_sort_key(DetailsSortKey::Name);
                harness.run();
                assert_ne!(
                    harness.state().details_order,
                    harness.state().visible_indices
                );
                check_buttons_against_keys(&mut harness);
            }
        }
    }
}

#[test]
fn section1855_empty_grid_and_modal_clicks_do_not_move_the_grid() {
    for facet in [false, true] {
        let mut harness = harness(facet, GridViewMode::Thumbnail);
        harness.run();
        let before = harness.state().selected;
        harness.state_mut().show_toolbar_reset_confirm = true;
        harness.run();
        for label in ["先頭", "末尾"] {
            assert!(harness.get_by_label(label).accesskit_node().is_disabled());
            harness.get_by_label(label).click();
            harness.run();
            assert_eq!(harness.state().selected, before);
        }
        harness.get_by_label("キャンセル").click();
        harness.run();
        harness
            .state_mut()
            .install_new_items(Vec::new(), Vec::new());
        // install_new_items replaces the source arrays; its callers then rebuild
        // the visible projection and redirect/clear the old cursor.
        harness.state_mut().rebuild_visible_indices();
        harness
            .state_mut()
            .finish_page_identity_preparation_for_test();
        harness.run();
        assert!(harness.state().current_grid_order().is_empty());
        assert_eq!(harness.state().selected, None, "empty before either click");
        for label in ["先頭", "末尾"] {
            harness.get_by_label(label).click();
            harness.run();
            assert_eq!(harness.state().selected, None);
            assert!(harness.state().current_grid_order().is_empty());
            assert!(!harness.state().grid_scroll.reveal_requested());
        }
    }
}

#[test]
fn section1855_unfocused_main_window_does_not_accept_edge_clicks() {
    let mut harness = harness(true, GridViewMode::Thumbnail);
    harness.run();
    let before = harness.state().selected;
    harness
        .input_mut()
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .focused = Some(false);
    harness.run();
    for label in ["先頭", "末尾"] {
        assert!(harness.get_by_label(label).accesskit_node().is_disabled());
        harness.get_by_label(label).click();
        harness.run();
        assert_eq!(harness.state().selected, before);
    }
    harness
        .input_mut()
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .focused = Some(true);
    harness.run();
    harness.get_by_label("末尾").click();
    harness.run();
    assert_ne!(harness.state().selected, before);
}

fn assert_visibility_saved(harness: &Harness<'_, phase_c_support::AppTestEnv>, visible: bool) {
    assert_eq!(harness.state().settings.show_grid_edge_buttons, visible);
    let saved = crate::settings_db::SettingsDb::open(&crate::data_dir::get())
        .unwrap()
        .load_into_settings()
        .unwrap();
    assert_eq!(saved.show_grid_edge_buttons, visible);
}

#[test]
fn section1855_right_click_toggle_saves_and_shows_or_hides_buttons() {
    for site in ["background", "toolbar_sort", "facet_sort", "first", "last"] {
        let mut harness = harness(site == "facet_sort", GridViewMode::Thumbnail);
        let initially_visible = matches!(site, "first" | "last");
        harness.state_mut().settings.show_grid_edge_buttons = initially_visible;
        harness.run();
        match site {
            "background" => {
                // toolbar_content_rect ends at the last control (the refresh
                // button here), so right()-8 is not background. Use the real
                // gap between the sort combo and refresh, outside both controls.
                let combo = harness.get_by_role(egui::accesskit::Role::ComboBox).rect();
                let refresh = harness.get_by_label("最新の情報に更新").rect();
                assert!(
                    combo.right() < refresh.left(),
                    "toolbar must have an item gap"
                );
                let pos = egui::pos2((combo.right() + refresh.left()) * 0.5, combo.center().y);
                assert!(harness.state().toolbar_content_rect.unwrap().contains(pos));
                assert!(!combo.contains(pos) && !refresh.contains(pos));
                harness.hover_at(pos);
                for pressed in [true, false] {
                    harness.event(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    });
                    harness.step();
                }
            }
            "first" => harness.get_by_label("先頭").click_secondary(),
            "last" => harness.get_by_label("末尾").click_secondary(),
            _ => {
                // ComboBox and label intentionally share accessibility text.
                let labels: Vec<_> = harness
                    .get_all_by_label("ソート:")
                    .into_iter()
                    .filter(|node| node.accesskit_node().role() == egui::accesskit::Role::Label)
                    .collect();
                labels.last().unwrap().click_secondary();
            }
        }
        harness.run();
        assert!(
            harness.query_by_label(VISIBILITY).is_some(),
            "site={site}: visibility menu did not open"
        );
        harness.get_by_label(VISIBILITY).click();
        harness.run();
        assert_visibility_saved(&harness, !initially_visible);
        egui::Popup::close_all(&harness.ctx);
        harness.run();
        assert_eq!(harness.query_by_label("先頭").is_some(), !initially_visible);
        assert_eq!(harness.query_by_label("末尾").is_some(), !initially_visible);
    }
}

fn open_toolbar_sort_menu(harness: &mut Harness<'_, phase_c_support::AppTestEnv>) {
    // 絞り込みバーにも同名のラベルがあるため、先に描画される上部のラベルを選ぶ。
    harness
        .get_all_by_label("ソート:")
        .into_iter()
        .find(|node| node.accesskit_node().role() == egui::accesskit::Role::Label)
        .expect("上部ツールバーのソートのラベル")
        .click_secondary();
    harness.run();
    assert!(harness.query_by_label(VISIBILITY).is_some());
}

#[test]
fn section1868_toolbar_sort_menu_explains_facet_placement_and_toggle_hides_note() {
    let mut harness = harness(true, GridViewMode::Thumbnail);
    harness.run();
    open_toolbar_sort_menu(&mut harness);
    assert!(harness.query_by_label(FACET_PLACEMENT_NOTE).is_some());
    harness.get_by_label(VISIBILITY).click();
    harness.run();
    assert!(!harness.state().settings.show_grid_edge_buttons);
    // メニューを閉じ直さなくても、チェックを外した描画で説明が消える。
    assert!(harness.query_by_label(VISIBILITY).is_some());
    assert!(harness.query_by_label(FACET_PLACEMENT_NOTE).is_none());
}

#[test]
fn section1868_toolbar_sort_menu_has_no_facet_note_when_facet_sort_or_bar_is_hidden() {
    for facet_bar_visible in [true, false] {
        let mut harness = harness(facet_bar_visible, GridViewMode::Thumbnail);
        // バーだけを隠した場合と、バーは出したままソートだけを隠した場合を分ける。
        harness.state_mut().settings.show_facet_sort = !facet_bar_visible;
        harness.run();
        assert_eq!(harness.get_all_by_label("先頭").count(), 1);
        open_toolbar_sort_menu(&mut harness);
        assert!(harness.query_by_label(FACET_PLACEMENT_NOTE).is_none());
    }
}

#[test]
fn section1868_toolbar_sort_menu_has_no_facet_note_when_edge_buttons_are_hidden() {
    let mut harness = harness(true, GridViewMode::Thumbnail);
    harness.state_mut().settings.show_grid_edge_buttons = false;
    harness.run();
    open_toolbar_sort_menu(&mut harness);
    assert!(harness.query_by_label(FACET_PLACEMENT_NOTE).is_none());
}
