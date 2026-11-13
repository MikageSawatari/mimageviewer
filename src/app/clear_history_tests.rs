use super::*;
use crate::ring_shortcut::{RingActionId, RingShortcutContext};
use crate::ui_pinned_search::PinnedSearchKind;

fn toast(app: &App) -> Option<&str> {
    app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str())
}

fn seed_reading(app: &App, number: usize) {
    for index in 0..number {
        app.reading_history_db
            .as_ref()
            .unwrap()
            .upsert(
                crate::reading_history_db::ReadingHistoryEntry::new(
                    PathBuf::from(format!("C:/clear-history/{index}.zip")),
                    crate::reading_history_db::ReadingHistoryKind::Zip,
                    None,
                    format!("{index}.zip"),
                    Some(1),
                    Some(2),
                ),
                1000,
            )
            .unwrap();
    }
}

fn reading_count(app: &App) -> usize {
    app.reading_history_db.as_ref().unwrap().count()
}

fn press(app: &mut App, ctx: &egui::Context, key: egui::Key, dialog: bool) {
    ctx.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    if dialog {
        app.show_reading_history_clear_confirm_dialog(ctx);
    } else {
        let _ = app.handle_keyboard(ctx);
    }
    ctx.end_pass().drop_without_applying_deltas();
}

#[test]
fn search_input_history_clear_key_persists_and_notifies_count() {
    let mut app = setup_app_for_test();
    for kind in PinnedSearchKind::ALL {
        app.settings.search_input_history.record(kind, "sentinel");
    }
    app.settings.save();
    app.keymap = crate::keymap::Keymap::from_ini_str("[Grid]\nGridClearSearchInputHistory = F13\n");
    press(&mut app, &egui::Context::default(), egui::Key::F13, false);
    assert_eq!(app.settings.search_input_history.len(), 0);
    let saved = crate::settings_db::with_db_result(|db| db.load_into_settings()).unwrap();
    assert_eq!(saved.search_input_history.len(), 0);
    assert_eq!(toast(&app), Some("検索欄の入力履歴を 4 件消去しました"));
}

#[test]
fn search_input_history_clear_empty_key_only_notifies_without_save() {
    let mut app = setup_app_for_test();
    let generation = crate::settings::save_generation();
    app.apply_history_clear_key_action(
        &egui::Context::default(),
        KeyAction::GridClearSearchInputHistory,
        "test",
    );
    assert_eq!(crate::settings::save_generation(), generation);
    assert_eq!(toast(&app), Some("検索欄の入力履歴はありません"));
}

#[test]
fn clear_history_keys_are_blocked_by_preferences_modal() {
    let mut app = setup_app_for_test();
    app.settings
        .search_input_history
        .record(PinnedSearchKind::Local, "keep");
    seed_reading(&app, 1);
    app.keymap = crate::keymap::Keymap::from_ini_str(
        "[Grid]\nGridClearSearchInputHistory = F13\nGridClearReadingHistory = F14\n",
    );
    app.show_preferences = true;
    assert!(app.common_modal_dialog_open());
    press(&mut app, &egui::Context::default(), egui::Key::F13, false);
    press(&mut app, &egui::Context::default(), egui::Key::F14, false);
    assert_eq!(app.settings.search_input_history.len(), 1);
    assert_eq!(reading_count(&app), 1);
    assert!(!app.reading_history_clear_dialog_visible());
    assert!(toast(&app).is_none());
}

#[test]
fn reading_history_clear_key_opens_modal_and_escape_cancels() {
    let mut app = setup_app_for_test();
    seed_reading(&app, 2);
    app.keymap = crate::keymap::Keymap::from_ini_str("[Grid]\nGridClearReadingHistory = F13\n");
    let ctx = egui::Context::default();
    press(&mut app, &ctx, egui::Key::F13, false);
    assert!(app.reading_history_clear_dialog_visible());
    assert!(app.common_modal_dialog_open());
    assert_eq!(reading_count(&app), 2);
    press(&mut app, &ctx, egui::Key::Escape, true);
    assert!(!app.common_modal_dialog_open());
    assert_eq!(reading_count(&app), 2);
    assert!(toast(&app).is_none());
}

#[test]
fn reading_history_clear_confirm_enter_deletes_and_recounts() {
    let mut app = setup_app_for_test();
    seed_reading(&app, 1);
    app.apply_history_clear_key_action(
        &egui::Context::default(),
        KeyAction::GridClearReadingHistory,
        "test",
    );
    seed_reading(&app, 3);
    press(&mut app, &egui::Context::default(), egui::Key::Enter, true);
    assert!(!app.common_modal_dialog_open());
    assert_eq!(reading_count(&app), 0);
    assert_eq!(toast(&app), Some("閲覧履歴を 3 件削除しました"));
}

#[test]
fn reading_history_clear_empty_key_does_not_open_modal_or_save() {
    let mut app = setup_app_for_test();
    let generation = crate::settings::save_generation();
    app.apply_history_clear_key_action(
        &egui::Context::default(),
        KeyAction::GridClearReadingHistory,
        "test",
    );
    assert!(!app.common_modal_dialog_open());
    assert_eq!(crate::settings::save_generation(), generation);
    assert_eq!(toast(&app), Some("閲覧履歴はありません"));
}

#[test]
fn reading_history_clear_enter_binding_repeat_cannot_confirm() {
    let mut app = setup_app_for_test();
    seed_reading(&app, 1);
    app.keymap = crate::keymap::Keymap::from_ini_str(
        "[Grid]\nGridClearReadingHistory = Enter\nGridOpenSelected = none\n",
    );
    let ctx = egui::Context::default();
    press(&mut app, &ctx, egui::Key::Enter, false);
    assert!(app.common_modal_dialog_open());
    ctx.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: true,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    app.show_reading_history_clear_confirm_dialog(&ctx);
    ctx.end_pass().drop_without_applying_deltas();
    assert!(app.common_modal_dialog_open());
    assert_eq!(reading_count(&app), 1);
    ctx.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    app.show_reading_history_clear_confirm_dialog(&ctx);
    ctx.end_pass().drop_without_applying_deltas();
    press(&mut app, &ctx, egui::Key::Enter, true);
    assert!(!app.common_modal_dialog_open());
    assert_eq!(reading_count(&app), 0);
}

#[test]
fn reading_history_clear_became_empty_before_confirmation() {
    let mut app = setup_app_for_test();
    seed_reading(&app, 1);
    app.request_clear_reading_history();
    app.reading_history_db
        .as_ref()
        .unwrap()
        .clear_all()
        .unwrap();
    app.resolve_reading_history_clear_confirmation(true);
    assert!(!app.common_modal_dialog_open());
    assert_eq!(toast(&app), Some("閲覧履歴はありません"));
}

#[test]
fn reading_history_clear_reloads_shown_history_view() {
    let mut app = setup_app_for_test();
    seed_reading(&app, 2);
    app.enter_reading_history();
    assert!(app.items_are_reading_history_view);
    let generation = app.items_generation;
    app.request_clear_reading_history();
    app.resolve_reading_history_clear_confirmation(true);
    assert_eq!(reading_count(&app), 0);
    assert!(app.items_are_reading_history_view);
    assert_ne!(app.items_generation, generation);
    assert!(app.items.is_empty());
    assert_eq!(toast(&app), Some("閲覧履歴を 2 件削除しました"));
}

#[test]
fn clear_history_ring_uses_same_commands() {
    let mut app = setup_app_for_test();
    app.settings
        .search_input_history
        .record(PinnedSearchKind::Local, "clear");
    seed_reading(&app, 1);
    let ctx = egui::Context::default();
    app.apply_ring_action(
        &ctx,
        RingShortcutContext::Grid,
        RingActionId::ClearSearchInputHistory,
        "test-ring",
    );
    assert_eq!(toast(&app), Some("検索欄の入力履歴を 1 件消去しました"));
    let saved = crate::settings_db::with_db_result(|db| db.load_into_settings()).unwrap();
    assert_eq!(saved.search_input_history.len(), 0);
    app.apply_ring_action(
        &ctx,
        RingShortcutContext::Grid,
        RingActionId::ClearReadingHistory,
        "test-ring",
    );
    assert!(app.common_modal_dialog_open());
    app.resolve_reading_history_clear_confirmation(false);
    assert_eq!(reading_count(&app), 1);
    app.apply_ring_action(
        &ctx,
        RingShortcutContext::Grid,
        RingActionId::ClearReadingHistory,
        "test-ring",
    );
    app.resolve_reading_history_clear_confirmation(true);
    assert_eq!(reading_count(&app), 0);
    assert_eq!(toast(&app), Some("閲覧履歴を 1 件削除しました"));
}

#[test]
fn reading_history_clear_failure_reports_error() {
    let mut app = setup_app_for_test();
    app.reading_history_db = None;
    app.request_clear_reading_history();
    app.resolve_reading_history_clear_confirmation(true);
    assert!(!app.common_modal_dialog_open());
    assert_eq!(toast(&app), Some("閲覧履歴の削除に失敗しました"));
}

fn hold_reading_history_exclusive_lock() -> rusqlite::Connection {
    let lock =
        rusqlite::Connection::open(crate::reading_history_db::ReadingHistoryDb::db_path()).unwrap();
    lock.execute_batch("BEGIN EXCLUSIVE").unwrap();
    lock
}

#[test]
fn reading_history_clear_key_count_failure_warns_without_opening_dialog() {
    let mut app = setup_app_for_test();
    seed_reading(&app, 2);
    let generation = app.items_generation;
    let lock = hold_reading_history_exclusive_lock();
    app.apply_history_clear_key_action(
        &egui::Context::default(),
        KeyAction::GridClearReadingHistory,
        "test",
    );
    assert!(!app.common_modal_dialog_open());
    assert_eq!(app.items_generation, generation);
    assert_eq!(toast(&app), Some("閲覧履歴の件数を確認できませんでした"));
    lock.execute_batch("ROLLBACK").unwrap();
    assert_eq!(reading_count(&app), 2);
}

#[test]
fn reading_history_clear_confirm_count_failure_warns_without_deleting() {
    let mut app = setup_app_for_test();
    seed_reading(&app, 2);
    app.request_clear_reading_history();
    assert!(app.common_modal_dialog_open());
    let generation = app.items_generation;
    let lock = hold_reading_history_exclusive_lock();
    app.resolve_reading_history_clear_confirmation(true);
    assert!(!app.common_modal_dialog_open());
    assert_eq!(app.items_generation, generation);
    assert_eq!(toast(&app), Some("閲覧履歴の件数を確認できませんでした"));
    lock.execute_batch("ROLLBACK").unwrap();
    assert_eq!(reading_count(&app), 2);
}

fn history_clear_grid_harness() -> egui_kittest::Harness<'static, AppTestEnvForTest> {
    let mut app = setup_app_for_test();
    seed_reading(&app, 2);
    let folder = app.tmp.path().join("modal-grid");
    app.current_folder = Some(folder.clone());
    app.items = (0..80)
        .map(|index| crate::grid_item::GridItem::ZipFile(folder.join(format!("book-{index}.zip"))))
        .collect();
    app.visible_indices = (0..80).collect();
    app.thumbnails = vec![crate::grid_item::ThumbnailState::Pending; 80];
    app.image_metas = vec![None; 80];
    app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
    app.settings.grid_cols = 4;
    app.settings.grid_click_selection_mode = crate::settings::GridClickSelectionMode::Explorer;
    app.settings.grid_open_selected_item_on_click = false;
    let mut fonts_ready = false;
    crate::test_ui::builder()
        .with_size(egui::vec2(1200.0, 480.0))
        .with_step_dt(0.01)
        .build_ui_state(
            move |root_ui, app| {
                let ctx = &root_ui.ctx().clone();
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                // Match App::update: dialogs precede the main grid's input and drawing.
                app.show_reading_history_clear_confirm_dialog(ctx);
                assert!(app.handle_keyboard(ctx).is_none());
                app.process_scroll(ctx);
                assert!(app.render_grid(root_ui).is_none());
            },
            app,
        )
}

fn click_history_grid_at(
    harness: &mut egui_kittest::Harness<'_, AppTestEnvForTest>,
    pos: egui::Pos2,
) {
    harness.hover_at(pos);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    harness.step();
}

fn wheel_history_grid(harness: &mut egui_kittest::Harness<'_, AppTestEnvForTest>) {
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -180.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
    });
    harness.run_steps(8);
}

fn key_history_grid(harness: &mut egui_kittest::Harness<'_, AppTestEnvForTest>, key: egui::Key) {
    for pressed in [true, false] {
        harness.event(egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.step();
    }
    harness.run_steps(4);
}

#[test]
fn reading_history_clear_modal_blocks_grid_click_wheel_and_keys() {
    use egui_kittest::kittest::Queryable;
    let mut harness = history_clear_grid_harness();
    harness.run_steps(6);
    let tile = harness
        .ctx
        .data(|data| {
            data.get_temp::<egui::Response>(egui::Id::unique((
                "grid_cell_input_response",
                harness.state().grid_scroll_context_id(),
                0usize,
            )))
        })
        .expect("real grid tile")
        .rect
        .center();
    // Positive controls: this point and these input handlers operate the actual grid.
    harness.state_mut().selected = Some(1);
    click_history_grid_at(&mut harness, tile);
    assert_eq!(harness.state().selected, Some(0));
    key_history_grid(&mut harness, egui::Key::ArrowRight);
    assert_eq!(harness.state().selected, Some(1));
    wheel_history_grid(&mut harness);
    assert!(harness.state().grid_scroll.offset() > 0.0);
    harness.state_mut().grid_scroll.set_position(0.0);
    harness.run_steps(6);

    harness.state_mut().request_clear_reading_history();
    harness.run_steps(6);
    let selected = harness.state().selected;
    let offset = harness.state().grid_scroll.offset();
    click_history_grid_at(&mut harness, tile);
    assert_eq!(
        harness.state().selected,
        selected,
        "backdrop must block tile selection"
    );
    assert!(
        harness.state().reading_history_clear_dialog_visible(),
        "backdrop click must not dismiss"
    );
    wheel_history_grid(&mut harness);
    assert_eq!(
        harness.state().grid_scroll.offset(),
        offset,
        "backdrop must block wheel scrolling"
    );
    for key in [egui::Key::ArrowDown, egui::Key::PageDown] {
        key_history_grid(&mut harness, key);
        assert_eq!(harness.state().selected, selected);
        assert_eq!(harness.state().grid_scroll.offset(), offset);
        assert!(harness.state().common_modal_dialog_open());
    }
    assert_eq!(reading_count(harness.state()), 2);
    harness.get_by_label("キャンセル").click();
    harness.run_steps(6);
    assert!(!harness.state().common_modal_dialog_open());
    assert_eq!(harness.state().selected, selected);
    assert_eq!(harness.state().grid_scroll.offset(), offset);
}

#[test]
fn reading_history_clear_modal_cancel_and_clear_buttons_work() {
    use egui_kittest::kittest::Queryable;
    let mut harness = history_clear_grid_harness();
    harness.state_mut().request_clear_reading_history();
    harness.run_steps(6);
    harness.get_by_label("キャンセル").click();
    harness.run_steps(6);
    assert!(!harness.state().common_modal_dialog_open());
    assert_eq!(reading_count(harness.state()), 2);
    harness.state_mut().request_clear_reading_history();
    harness.run_steps(6);
    harness.get_by_label("クリア").click();
    harness.run_steps(6);
    assert!(!harness.state().common_modal_dialog_open());
    assert_eq!(reading_count(harness.state()), 0);
    assert_eq!(toast(harness.state()), Some("閲覧履歴を 2 件削除しました"));
}
