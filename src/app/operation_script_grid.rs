//! Resolved grid actions shared with the ordinary keyboard handlers.

use super::App;
use super::gamepad_input::GridSelectionEdge;
use super::operation_script_runtime::ScriptRunResult;
use crate::keymap::{KeyAction, RatingEdit, RatingStepDirection, RatingTargetFamily};
use crate::operation_script::{ALLOWED_OPERATIONS, ScriptContext};

pub(crate) fn script_rating_key_edit(
    action: KeyAction,
) -> Option<(RatingTargetFamily, RatingEdit)> {
    use KeyAction::*;
    use RatingTargetFamily::*;
    Some(match action {
        RatingItem1 => (Item, RatingEdit::Assign(1)),
        RatingItem2 => (Item, RatingEdit::Assign(2)),
        RatingItem3 => (Item, RatingEdit::Assign(3)),
        RatingItem4 => (Item, RatingEdit::Assign(4)),
        RatingItem5 => (Item, RatingEdit::Assign(5)),
        RatingItemClear => (Item, RatingEdit::Assign(0)),
        RatingItemStepUp => (Item, RatingEdit::Step(RatingStepDirection::Up)),
        RatingItemStepDown => (Item, RatingEdit::Step(RatingStepDirection::Down)),
        RatingContainer1 => (Container, RatingEdit::Assign(1)),
        RatingContainer2 => (Container, RatingEdit::Assign(2)),
        RatingContainer3 => (Container, RatingEdit::Assign(3)),
        RatingContainer4 => (Container, RatingEdit::Assign(4)),
        RatingContainer5 => (Container, RatingEdit::Assign(5)),
        RatingContainerClear => (Container, RatingEdit::Assign(0)),
        RatingContainerStepUp => (Container, RatingEdit::Step(RatingStepDirection::Up)),
        RatingContainerStepDown => (Container, RatingEdit::Step(RatingStepDirection::Down)),
        RatingBookPreferred1 => (BookPreferred, RatingEdit::Assign(1)),
        RatingBookPreferred2 => (BookPreferred, RatingEdit::Assign(2)),
        RatingBookPreferred3 => (BookPreferred, RatingEdit::Assign(3)),
        RatingBookPreferred4 => (BookPreferred, RatingEdit::Assign(4)),
        RatingBookPreferred5 => (BookPreferred, RatingEdit::Assign(5)),
        RatingBookPreferredClear => (BookPreferred, RatingEdit::Assign(0)),
        RatingBookPreferredStepUp => (BookPreferred, RatingEdit::Step(RatingStepDirection::Up)),
        RatingBookPreferredStepDown => (BookPreferred, RatingEdit::Step(RatingStepDirection::Down)),
        _ => return None,
    })
}

impl App {
    /// This is the existing column/details key body, without key probing.
    pub(crate) fn execute_grid_view_key_action(&mut self, action: KeyAction) -> bool {
        if action == KeyAction::GridToggleDetailsView {
            self.toggle_grid_details_view();
            return true;
        }
        const COLUMNS: [KeyAction; 20] = [
            KeyAction::GridColumnCount1,
            KeyAction::GridColumnCount2,
            KeyAction::GridColumnCount3,
            KeyAction::GridColumnCount4,
            KeyAction::GridColumnCount5,
            KeyAction::GridColumnCount6,
            KeyAction::GridColumnCount7,
            KeyAction::GridColumnCount8,
            KeyAction::GridColumnCount9,
            KeyAction::GridColumnCount10,
            KeyAction::GridColumnCount11,
            KeyAction::GridColumnCount12,
            KeyAction::GridColumnCount13,
            KeyAction::GridColumnCount14,
            KeyAction::GridColumnCount15,
            KeyAction::GridColumnCount16,
            KeyAction::GridColumnCount17,
            KeyAction::GridColumnCount18,
            KeyAction::GridColumnCount19,
            KeyAction::GridColumnCount20,
        ];
        let Some(cols) = COLUMNS
            .iter()
            .position(|candidate| *candidate == action)
            .map(|i| i + 1)
        else {
            return false;
        };
        self.set_grid_view_mode(crate::settings::GridViewMode::Thumbnail);
        if cols != self.settings.grid_cols {
            self.settings.grid_cols = cols;
            self.settings.save();
        }
        true
    }

    /// Execute an already-resolved allowlisted grid action. Navigation stays with
    /// the caller's existing AddressBarNav dispatcher.
    pub(crate) fn execute_grid_key_action(
        &mut self,
        ctx: &egui::Context,
        action: KeyAction,
        source: &'static str,
    ) -> ScriptRunResult {
        let result = |nav, rejected| ScriptRunResult::Grid { nav, rejected };
        if !ALLOWED_OPERATIONS
            .iter()
            .any(|entry| entry.action == action && entry.contexts.contains(ScriptContext::Grid))
        {
            return result(None, true);
        }
        if self.execute_grid_view_key_action(action) {
            return result(None, false);
        }
        if action.book_slot_number().is_some()
            || action.collection_slot_number().is_some()
            || matches!(action, KeyAction::GridBookPrev | KeyAction::GridBookNext)
        {
            return match self.try_apply_saved_group_key_action(ctx, action) {
                Ok(nav) => result(nav, false),
                Err(()) => result(None, true),
            };
        }
        if action.favorite_slot_number().is_some() {
            let nav = self.apply_location_navigation_key_action(ctx, action, source);
            let rejected = nav.is_none();
            return result(nav, rejected);
        }
        match action {
            KeyAction::GridParentFolder => {
                return result(
                    if self.grid_item_input_block().is_some() {
                        self.handle_stack_refresh_parent_action()
                    } else {
                        self.handle_grid_parent_folder_action()
                    },
                    false,
                );
            }
            KeyAction::GridHistoryBack | KeyAction::GridHistoryForward => {
                return result(
                    self.history_key_navigation(if action == KeyAction::GridHistoryBack {
                        super::FolderHistoryDirection::Back
                    } else {
                        super::FolderHistoryDirection::Forward
                    }),
                    false,
                );
            }
            _ => {}
        }
        if let KeyAction::GridSiblingFolderPrev | KeyAction::GridSiblingFolderNext = action {
            let forward = action == KeyAction::GridSiblingFolderNext;
            if self.grid_item_input_block().is_some() {
                self.execute_grid_refresh_book_navigation(
                    ctx,
                    forward,
                    super::FolderNavMode::SiblingGrid,
                );
            } else {
                self.handle_grid_book_navigation_keys(ctx, false, false, forward, !forward);
            }
            return result(None, false);
        }
        if !self.grid_item_input_allowed() {
            return result(None, true);
        }
        if let Some((family, edit)) = script_rating_key_edit(action) {
            let accepted = if family == RatingTargetFamily::Container {
                let noop = self.operation_script_rating_noop(family, None, edit);
                self.apply_rating_edit_to_current_container(edit) || noop
            } else if self.ratable_targets().is_empty() {
                false
            } else {
                self.apply_rating_edit_to_selection(edit);
                true
            };
            return result(None, !accepted);
        }
        if action.pinned_tag_slot_number().is_some() {
            let accepted = self.apply_pinned_tag_key_action(action, source);
            return result(None, !accepted);
        }
        match action {
            KeyAction::GridMoveFirst | KeyAction::GridMoveLast => {
                self.move_grid_selection_to_edge(
                    ctx,
                    if action == KeyAction::GridMoveFirst {
                        GridSelectionEdge::First
                    } else {
                        GridSelectionEdge::Last
                    },
                    source,
                );
                result(None, false)
            }
            KeyAction::GridOpenSelected => {
                if self.selected.and_then(|idx| self.items.get(idx)).is_none() {
                    return result(None, true);
                }
                result(self.execute_grid_open_selected(ctx, false), false)
            }
            KeyAction::GridCopyPath | KeyAction::GridCopyFileName | KeyAction::GridCopyImage => {
                let accepted = self.execute_copy_key_action(ctx, action);
                result(None, !accepted)
            }
            _ => result(None, true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid_item::GridItem;
    use crate::operation_script::{CompiledOperationScript, OperationScriptState};
    use crate::ring_shortcut::{MouseButtonSlot, RingActionId, RingDirection, RingShortcutContext};

    fn fixture(operations: &str) -> crate::app::AppTestEnvForTest {
        let mut app = crate::app::setup_app_for_test();
        app.items = (0..3)
            .map(|i| GridItem::Image(format!("C:/script-grid/{i}.png").into()))
            .collect();
        app.visible_indices = vec![0, 1, 2];
        app.selected = Some(1);
        app.current_folder = Some("C:/script-grid".into());
        app.settings.operation_script_enabled = true;
        app.operation_script.state = OperationScriptState::Ready(Box::new(
            CompiledOperationScript::compile(&format!(
                "fn slots() {{ [#{{slot:1,label:\"test\",run:\"run\"}}] }} fn run(s) {{ [{operations}] }}"
            )).unwrap(),
        ));
        app
    }

    fn key_frame(ctx: &egui::Context, action: impl FnOnce()) {
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::F13,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        });
        action();
        ctx.end_pass().drop_without_applying_deltas();
    }

    #[test]
    fn script_grid_column_and_details_actions_share_the_key_handler() {
        for operation in ALLOWED_OPERATIONS.iter().filter(|entry| {
            entry.contexts.contains(ScriptContext::Grid)
                && (entry.action.ini_name().starts_with("GridColumnCount")
                    || entry.action == KeyAction::GridToggleDetailsView)
        }) {
            let expected = {
                let mut key_app = fixture("");
                key_app.keymap = crate::keymap::Keymap::from_ini_str(&format!(
                    "[Grid]\n{} = F13\n",
                    operation.action.ini_name()
                ));
                let key_ctx = egui::Context::default();
                key_frame(&key_ctx, || {
                    key_app.input_main_grid_shortcuts(&key_ctx, false);
                });
                (key_app.settings.grid_cols, key_app.settings.grid_view_mode)
            };
            let mut script_app = fixture(&format!("\"{}\"", operation.action.ini_name()));
            let result = script_app.run_script_slot(
                &egui::Context::default(),
                1,
                super::super::operation_script_runtime::ScriptInputSurface::Grid,
                false,
            );
            assert!(matches!(
                result,
                ScriptRunResult::Grid {
                    nav: None,
                    rejected: false
                }
            ));
            assert_eq!(
                expected.0, script_app.settings.grid_cols,
                "{:?}",
                operation.action
            );
            assert_eq!(
                expected.1, script_app.settings.grid_view_mode,
                "{:?}",
                operation.action
            );
        }
    }

    #[test]
    fn script_grid_edge_action_matches_keyboard_and_all_ring_routes() {
        for (key_action, ring_action, expected) in [
            (KeyAction::GridMoveFirst, RingActionId::GridMoveFirst, 0),
            (KeyAction::GridMoveLast, RingActionId::GridMoveLast, 2),
        ] {
            let body = format!("\"{}\"", key_action.ini_name());
            {
                let mut key_app = fixture(&body);
                key_app.keymap = crate::keymap::Keymap::from_ini_str(&format!(
                    "[Grid]\n{} = F13\n",
                    key_action.ini_name()
                ));
                let key_ctx = egui::Context::default();
                key_frame(&key_ctx, || {
                    key_app.handle_keyboard(&key_ctx);
                });
                assert_eq!(key_app.selected, Some(expected));
            }
            for route in 0..4 {
                let mut app = fixture(&body);
                let ctx = egui::Context::default();
                let nav = match route {
                    0 => app.apply_ring_action(
                        &ctx,
                        RingShortcutContext::Grid,
                        ring_action.clone(),
                        "grid-parity",
                    ),
                    1 => app.apply_ring_action(
                        &ctx,
                        RingShortcutContext::Grid,
                        RingActionId::ScriptSlot1,
                        "script-ring",
                    ),
                    2 => {
                        app.settings
                            .ring_shortcuts
                            .mouse_button_profile_mut(RingShortcutContext::Grid)
                            .back = RingActionId::ScriptSlot1;
                        app.dispatch_mouse_button(
                            &ctx,
                            MouseButtonSlot::Back,
                            super::super::ActionSurface::MainWindow,
                            "script-mouse",
                        )
                        .navigation
                    }
                    _ => {
                        app.settings
                            .ring_shortcuts
                            .profile_mut(RingShortcutContext::Grid)
                            .slots[RingDirection::Up.slot_index()] = RingActionId::ScriptSlot1;
                        app.trigger_gamepad_ring_action_for_test(&ctx, RingDirection::Up)
                    }
                };
                assert!(nav.is_none());
                assert_eq!(
                    app.selected,
                    Some(expected),
                    "route={route} action={key_action:?}"
                );
            }
        }
    }

    #[test]
    fn script_grid_rejected_tag_and_copy_stop_before_later_actions() {
        for first in ["GridTogglePinnedTag20", "GridCopyImage"] {
            let mut app = fixture(&format!("\"{first}\",\"GridMoveLast\""));
            // Folder cannot be copied as an image; slot 20 is deliberately unregistered.
            app.items[1] = GridItem::Folder("C:/script-grid/folder".into());
            app.settings.tags.clear();
            let result = app.run_script_slot(
                &egui::Context::default(),
                1,
                super::super::operation_script_runtime::ScriptInputSurface::Grid,
                false,
            );
            assert!(matches!(
                result,
                ScriptRunResult::Grid {
                    nav: None,
                    rejected: true
                }
            ));
            assert_eq!(app.selected, Some(1));
        }
    }
}
