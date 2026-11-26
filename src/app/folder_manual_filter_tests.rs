//! Manual reorder must retain the filters that supplied its visible subset.
use super::*;
use crate::folder_order::{FolderOrderMode, FolderOrderRecord, folder_key, physical_item_key};

const NAMES: [&str; 4] = [
    "a-keep-red.png",
    "b-keep-blue.png",
    "c-other-red.png",
    "d-keep-red.png",
];

fn fixture(manual: bool) -> (AppTestEnvForTest, PathBuf) {
    let mut app = setup_app_for_test();
    app.settings.sidecar_backup_enabled = false;
    app.settings.tag_sidecar_backup_enabled = false;
    app.settings.auto_fullscreen_image_folders = false;
    app.settings.show_toolbar_search_local = false;
    app.active_quick_folder_slot = None;
    app.settings.sort_order = crate::settings::SortOrder::FileName;
    app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
    let folder = app.tmp.path().join("manual-filter");
    std::fs::create_dir(&folder).unwrap();
    for (index, name) in NAMES.into_iter().enumerate() {
        let rgb = if index == 1 { [0, 0, 255] } else { [255, 0, 0] };
        image::RgbImage::from_pixel(8, 8, image::Rgb(rgb))
            .save(folder.join(name))
            .unwrap();
    }
    std::fs::write(folder.join("hidden.txt"), b"inventory only").unwrap();
    app.load_folder(folder.clone());
    app.finish_page_identity_preparation_for_test();
    if manual {
        app.enter_folder_manual_order();
        finish_refresh(&mut app);
    }
    (app, folder)
}

fn keys(app: &App) -> Vec<String> {
    app.items.iter().filter_map(physical_item_key).collect()
}

fn visible_keys(app: &App) -> Vec<String> {
    app.visible_indices
        .iter()
        .map(|&index| physical_item_key(&app.items[index]).unwrap())
        .collect()
}

fn read_record(app: &App, folder: &Path) -> Option<FolderOrderRecord> {
    app.folder_order_handle()
        .unwrap()
        .read(folder)
        .unwrap()
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap()
}

fn finish_filters(app: &mut App) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        app.poll_sidecar_restore(&ctx);
        app.poll_search(&ctx);
        app.poll_color_scan(&ctx);
        if !app.sidecar_restore_active()
            && app.search_pending.is_none()
            && app.color_filter.pending.is_none()
            && !app.color_filter_scope_refresh_pending
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "manual reorder filter workers did not settle"
        );
        std::thread::yield_now();
    }
}

fn finish_refresh(app: &mut App) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        app.poll_sidecar_restore(&ctx);
        if let Some(ready) = app.poll_folder_pane_open(&ctx) {
            let FolderOpenScanPurpose::CurrentViewOrderRefresh {
                order,
                reload_owner,
            } = ready.purpose
            else {
                panic!("unexpected folder open purpose");
            };
            assert!(app.apply_current_view_order_refresh(
                ready.path,
                ready.scan.unwrap(),
                order,
                *reload_owner
            ));
            app.finish_page_identity_preparation_for_test();
            finish_filters(app);
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "manual reorder refresh did not finish"
        );
        std::thread::yield_now();
    }
}

fn apply_filters(app: &mut App, color: bool, search: bool) {
    let ctx = egui::Context::default();
    if search {
        app.show_search_bar = true;
        app.search_query = "keep".into();
        app.search_target =
            crate::fts_index::SearchTarget::Only(vec![crate::fts_index::SourceKind::Filename]);
        app.search_or_mode = false;
        app.execute_search(&ctx);
        finish_filters(app);
        // A refresh must replay the submitted conditions, not submit this new draft.
        app.search_query = "unsubmitted replacement".into();
    }
    if color {
        app.color_filter.tolerance = crate::color_search::MIN_TOLERANCE;
        app.apply_image_color_filter_from_swatch([255, 0, 0], &ctx);
        finish_filters(app);
        assert!(app.color_filter.applied_scope_signature.is_some());
    }
}

fn filtered_reorder_preserves_membership(color: bool, search: bool) {
    let (mut app, folder) = fixture(true);
    let [a, b, c, d] = NAMES.map(|name| folder_key(&folder.join(name)));
    let initial = vec![a.clone(), b.clone(), c.clone(), d.clone()];
    assert_eq!(keys(&app), initial);
    assert!(read_record(&app, &folder).is_none());
    apply_filters(&mut app, color, search);
    let (visible, reordered, expected) = match (color, search) {
        (true, false) => (
            vec![a.clone(), c.clone(), d.clone()],
            vec![d.clone(), a.clone(), c.clone()],
            vec![d.clone(), b.clone(), a.clone(), c.clone()],
        ),
        (false, true) => (
            vec![a.clone(), b.clone(), d.clone()],
            vec![d.clone(), a.clone(), b.clone()],
            vec![d.clone(), a.clone(), c.clone(), b.clone()],
        ),
        (true, true) => (
            vec![a.clone(), d.clone()],
            vec![d.clone(), a.clone()],
            vec![d.clone(), b.clone(), c.clone(), a.clone()],
        ),
        _ => unreachable!(),
    };
    assert_eq!(visible_keys(&app), visible);
    let palette_keys = app
        .color_filter
        .palettes
        .map
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    app.move_grid_cursor(Some(0), GridCursorIntent::Automatic);
    app.checked = [
        *app.visible_indices.first().unwrap(),
        *app.visible_indices.last().unwrap(),
    ]
    .into_iter()
    .collect();
    let expected_checked = app
        .checked
        .iter()
        .map(|&index| physical_item_key(&app.items[index]).unwrap())
        .collect::<HashSet<_>>();
    let generation = app.items_generation;
    app.commit_folder_manual_reorder(&reordered);
    let saved = read_record(&app, &folder).unwrap();
    assert!(saved.active);
    assert_eq!(saved.items, expected);
    assert_eq!(keys(&app), initial);
    assert_eq!(app.items_generation, generation);
    finish_refresh(&mut app);
    assert!(app.items_generation > generation);
    assert_eq!(app.folder_order.mode, FolderOrderMode::ManualRecorded);
    assert_eq!(keys(&app), expected);
    assert_eq!(visible_keys(&app), reordered);
    assert_eq!(app.color_filter.enabled, color);
    if color {
        assert_eq!(app.color_filter.query_rgb, [255, 0, 0]);
        assert_eq!(
            app.color_filter.tolerance,
            crate::color_search::MIN_TOLERANCE
        );
        assert!(app.color_filter.applied_scope_signature.is_some());
        assert!(!palette_keys.is_empty());
        assert!(
            palette_keys
                .iter()
                .all(|key| app.color_filter.palettes.map.contains_key(key))
        );
    }
    if search {
        assert!(app.show_search_bar);
        assert_eq!(app.search_query, "unsubmitted replacement");
        let submitted = &app.search_filter.as_ref().unwrap().submitted;
        assert_eq!(submitted.query, "keep");
        assert!(!submitted.or_mode);
        assert!(matches!(
            &submitted.target,
            crate::fts_index::SearchTarget::Only(kinds)
                if kinds.as_slice() == [crate::fts_index::SourceKind::Filename]
        ));
    }
    assert_eq!(
        app.selected
            .and_then(|index| physical_item_key(&app.items[index])),
        Some(a.clone())
    );
    let checked = app
        .checked
        .iter()
        .map(|&index| physical_item_key(&app.items[index]).unwrap())
        .collect::<HashSet<_>>();
    assert_eq!(checked, expected_checked);
    assert!(
        app.folder_order
            .inventory
            .contains(&folder_key(&folder.join("hidden.txt")))
    );
    assert_eq!(read_record(&app, &folder).unwrap().items, expected);
}

#[test]
fn folder_manual_reorder_preserves_image_color_filter_after_refresh() {
    filtered_reorder_preserves_membership(true, false);
}

#[test]
fn folder_manual_reorder_preserves_ctrl_f_filter_and_draft_after_refresh() {
    filtered_reorder_preserves_membership(false, true);
}

#[test]
fn folder_manual_reorder_preserves_intersected_color_and_ctrl_f_filters_after_refresh() {
    filtered_reorder_preserves_membership(true, true);
}

#[test]
fn folder_standard_sort_refresh_retains_existing_filter_clear_contract() {
    let (mut app, folder) = fixture(false);
    apply_filters(&mut app, true, true);
    assert_eq!(visible_keys(&app).len(), 2);
    app.settings.sort_order = crate::settings::SortOrder::FileNameDesc;
    app.apply_sort_change_reload();
    finish_refresh(&mut app);
    assert!(!app.color_filter.enabled);
    assert!(app.color_filter.palettes.map.is_empty());
    assert!(app.search_filter.is_none());
    let expected = NAMES
        .into_iter()
        .rev()
        .map(|name| folder_key(&folder.join(name)))
        .collect::<Vec<_>>();
    assert_eq!(keys(&app), expected);
    assert_eq!(visible_keys(&app), expected);
}

#[test]
fn folder_manual_reorder_search_replay_does_not_change_explicit_search_cursor_reset() {
    let (mut app, folder) = fixture(true);
    apply_filters(&mut app, false, true);
    let [a, b, c, d] = NAMES.map(|name| folder_key(&folder.join(name)));
    app.move_grid_cursor(Some(0), GridCursorIntent::Automatic);
    app.commit_folder_manual_reorder(&[d.clone(), a.clone(), b]);
    finish_refresh(&mut app);
    assert_eq!(
        app.selected
            .and_then(|index| physical_item_key(&app.items[index])),
        Some(a.clone())
    );
    app.search_query = "red".into();
    app.execute_search(&egui::Context::default());
    finish_filters(&mut app);
    assert_eq!(visible_keys(&app), vec![d, a, c]);
    assert_eq!(app.selected, None);
}

#[test]
fn folder_manual_reorder_ctrl_f_replay_preserves_absent_cursor() {
    let (mut app, folder) = fixture(true);
    apply_filters(&mut app, false, true);
    let [a, b, c, d] = NAMES.map(|name| folder_key(&folder.join(name)));
    app.move_grid_cursor(None, GridCursorIntent::Automatic);
    assert_eq!(app.selected, None);
    app.commit_folder_manual_reorder(&[d.clone(), a.clone(), b.clone()]);
    finish_refresh(&mut app);
    assert_eq!(keys(&app), vec![d.clone(), a.clone(), c, b.clone()]);
    assert_eq!(visible_keys(&app), vec![d, a, b]);
    assert_eq!(app.selected, None);
}
