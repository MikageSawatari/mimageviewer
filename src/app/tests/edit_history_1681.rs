//! History input at the real main/fullscreen outer gates (no native windows).
use super::*;
use crate::keyboard_input::{DiscreteKeyInputSource, KeyboardOwner};

fn seed_history(app: &mut App) {
    // The payload is irrelevant to routing; the real handlers must move one
    // entry between stacks without metadata writes or background work.
    app.meta_undo
        .push_undo_from_redo(crate::undo_stack::UndoEntry::Adjustment {
            changes: Vec::new(),
            summary: "history routing".into(),
        });
}

fn raw(key: egui::Key, modifiers: egui::Modifiers, focused: bool) -> egui::RawInput {
    let mut raw = egui::RawInput {
        focused,
        events: vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers,
            },
        ],
        ..Default::default()
    };
    raw.viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .focused = Some(focused);
    raw
}

#[test]
#[cfg(windows)]
fn edit_history_root_routed_undo_redo_survives_focus_loss_but_modal_still_blocks() {
    let _input = crate::key_input::lock_test_input();
    for modal in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        seed_history(&mut app);
        app.show_open_folder_dialog = modal;
        let ctx = egui::Context::default();
        for (key, modifiers, expected) in [
            (egui::Key::Z, egui::Modifiers::CTRL, 0),
            (egui::Key::Y, egui::Modifiers::CTRL, 1),
            (egui::Key::Z, egui::Modifiers::CTRL, 0),
            (
                egui::Key::Z,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                1,
            ),
        ] {
            ctx.begin_pass(raw(key, modifiers, false));
            crate::key_input::set_test_frame(vec![crate::key_input::KeyEdge {
                operation_id: crate::key_input::KeyOperationId::unassigned(),
                source_hwnd: 1,
                source_viewport: egui::ViewportId::ROOT,
                virtual_key: if key == egui::Key::Z { 0x5A } else { 0x59 },
                scan_code: 0,
                extended: false,
                pressed: true,
                repeat: false,
                ctrl: true,
                shift: modifiers.shift,
                alt: false,
            }]);
            assert_eq!(
                app.keyboard_owner_for_pass(&ctx),
                if modal {
                    KeyboardOwner::Modal
                } else {
                    KeyboardOwner::Unclaimed
                }
            );
            assert_eq!(
                crate::keyboard_input::keyboard_input_permits(&ctx)
                    .discrete
                    .unwrap()
                    .source(),
                DiscreteKeyInputSource::RoutedKeyEdge
            );
            let _ = app.handle_keyboard(&ctx);
            assert_eq!(app.meta_undo.undo_len(), if modal { 1 } else { expected });
            assert_eq!(ctx.input(|i| i.key_pressed(key)), modal);
            ctx.end_pass().drop_without_applying_deltas();
            crate::key_input::clear_test_frame();
        }
    }
}

#[test]
fn edit_history_root_modeless_vst3_panel_keeps_fullscreen_undo_redo() {
    let _input = crate::key_input::lock_test_input();
    for modal in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        seed_history(&mut app);
        app.items = vec![GridItem::Audio(app.tmp.path().join("music.flac"))];
        app.fullscreen_idx = Some(0);
        app.viewer_presentation = ViewerPresentation::MainWindow;
        app.show_vst3_manager = true;
        app.show_open_folder_dialog = modal;
        let ctx = egui::Context::default();
        for (key, modifiers, expected) in [
            (egui::Key::Z, egui::Modifiers::CTRL, 0),
            (egui::Key::Y, egui::Modifiers::CTRL, 1),
            (egui::Key::Z, egui::Modifiers::CTRL, 0),
            (
                egui::Key::Z,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                1,
            ),
        ] {
            ctx.begin_pass(raw(key, modifiers, true));
            assert!(app.any_dialog_open());
            assert_eq!(app.any_modal_dialog_open_for_fullscreen_keys(), modal);
            assert_eq!(app.keyboard_owner_for_pass(&ctx), KeyboardOwner::Modal);
            app.handle_fs_key_input(&ctx, 0, false);
            assert_eq!(app.meta_undo.undo_len(), if modal { 1 } else { expected });
            assert_eq!(ctx.input(|i| i.key_pressed(key)), modal);
            ctx.end_pass().drop_without_applying_deltas();
        }
    }
}
