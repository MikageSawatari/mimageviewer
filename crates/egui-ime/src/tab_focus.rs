//! Shared host policy: Tab never traverses keyboard focus; its events remain available.

use std::sync::Arc;

const INSTALLATION_ID: &str = "miv_tab_shortcut_focus_policy_installed";

fn tab_pressed(ctx: &egui::Context) -> bool {
    ctx.input(|input| {
        input.events.iter().any(|event| {
            matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::Tab,
                    pressed: true,
                    ..
                }
            )
        })
    })
}

/// Cancel focus traversal that egui already derived from `Tab`.
pub fn cancel_tab_focus_traversal(ctx: &egui::Context) {
    ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
}

/// Install mimageviewer's `Tab` shortcut policy on an egui context.
///
/// The callback runs before the first focusable widget is registered, resets the
/// focus direction for every `Tab` press, and deliberately does not consume the
/// event. The focused UI or the later keymap ownership boundary still receives
/// the same press.
pub fn install_tab_shortcut_focus_policy(ctx: &egui::Context) {
    let installation_id = egui::Id::new(INSTALLATION_ID);
    let already_installed = ctx.data_mut(|data| {
        if data.get_temp::<bool>(installation_id).unwrap_or(false) {
            true
        } else {
            data.insert_temp(installation_id, true);
            false
        }
    });
    if already_installed {
        return;
    }

    ctx.on_begin_pass(
        "miv_tab_shortcut_focus_policy",
        Arc::new(|ctx| {
            if !tab_pressed(ctx) {
                return;
            }
            cancel_tab_focus_traversal(ctx);
        }),
    );
}
