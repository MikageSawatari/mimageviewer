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
