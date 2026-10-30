//! 消しゴム・隠蔽の結果プレビューに共通のキー保持受付。

use eframe::egui;

use crate::app::App;
use crate::keymap::{KeyAction, Keymap};

/// ボタンや処理要求とは分離し、描画中に押されたキーを実際の解放まで拒否する。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum EditPreviewHoldState {
    #[default]
    Released,
    Held,
    BlockedUntilRelease,
}

impl EditPreviewHoldState {
    /// Key handlers can finish/cancel drawing before the canvas sees this frame.
    /// Only reject here; activation stays after the pipeline to preserve simultaneous drawing.
    pub(crate) fn read_before_keys(
        &mut self,
        ctx: &egui::Context,
        keymap: &Keymap,
        action: KeyAction,
        drawing_in_progress: bool,
    ) {
        if drawing_in_progress {
            self.read(ctx, keymap, action, true);
        }
    }

    pub(crate) fn read(
        &mut self,
        ctx: &egui::Context,
        keymap: &Keymap,
        action: KeyAction,
        drawing_in_progress: bool,
    ) {
        // Legacy keymap helpers leave Modal to their callers (unlike TextInput).
        let permit = crate::keyboard_input::focused_key_state_permit(ctx).filter(|_| {
            crate::keyboard_input::cached_keyboard_owner(ctx)
                != Some(crate::keyboard_input::KeyboardOwner::Modal)
        });
        let held = permit.is_some_and(|permit| keymap.key_held_action(ctx, permit, action));
        let released = if drawing_in_progress || *self == Self::BlockedUntilRelease {
            permit.and_then(|permit| keymap.key_hold_binding_released(ctx, permit, action))
        } else {
            None
        };
        *self = match (*self, held, released, drawing_in_progress) {
            (_, _, Some(true), _) => Self::Released,
            (Self::BlockedUntilRelease, _, _, _) | (Self::Released, _, Some(false), true) => {
                Self::BlockedUntilRelease
            }
            (_, true, _, _) => Self::Held,
            _ => Self::Released,
        };
    }

    pub(crate) fn active(self, pointer_pressed: bool) -> bool {
        pointer_pressed || self == Self::Held
    }

    /// Canvas input precedes the panel: a simultaneous drawing press keeps its owner.
    pub(crate) fn merge(
        &mut self,
        pointer_pressed: bool,
        was_active: bool,
        drawing_in_progress: bool,
    ) -> bool {
        if !was_active && *self == Self::Held && drawing_in_progress {
            *self = Self::BlockedUntilRelease;
        }
        self.active(pointer_pressed)
    }
}

impl App {
    /// Navigation is resolved first. Preview owns any remaining canvas press until release.
    pub(crate) fn edit_preview_owns_canvas(&mut self, active: bool, primary_down: bool) -> bool {
        if active && primary_down {
            self.fs_primary_suppression.arm_pointer_stream();
        }
        active
    }
}
