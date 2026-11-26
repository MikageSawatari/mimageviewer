//! Confirmation belongs to the folder/context that requested it.

use eframe::egui;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ResetAction {
    #[default]
    None,
    Confirm,
    Cancel,
}

#[derive(Default)]
struct ResetDialogOutput {
    action: ResetAction,
    #[cfg(test)]
    geometry: Option<(egui::Rect, egui::Rect, egui::Rect)>,
}

fn draw_reset_confirmation(ctx: &egui::Context, enter: bool, escape: bool) -> ResetDialogOutput {
    let mut open = true;
    let mut output = ResetDialogOutput::default();
    let width = (ctx.content_rect().width() - 120.0).clamp(240.0, 440.0);
    #[cfg(test)]
    let mut buttons = None;
    let window = egui::Window::new("手動順のリセット")
        .id(egui::Id::new("folder_order_reset_confirm"))
        .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
        .default_width(width)
        .max_width(width)
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("このフォルダに保存された手動順を破棄し、現在の並び順からやり直しますか？");
            ui.colored_label(ui.visuals().warn_fg_color, "この操作は元に戻せません。");
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let confirm = ui.button("リセット");
                let cancel = ui.button("キャンセル");
                #[cfg(test)]
                {
                    buttons = Some((confirm.rect, cancel.rect));
                }
                output.action = if cancel.clicked() || escape {
                    ResetAction::Cancel
                } else if confirm.clicked() || enter {
                    ResetAction::Confirm
                } else {
                    ResetAction::None
                };
            });
        });
    #[cfg(test)]
    if let (Some(window), Some((confirm, cancel))) = (&window, buttons) {
        output.geometry = Some((window.response.rect, confirm, cancel));
    }
    #[cfg(not(test))]
    let _ = window;
    if !open || escape {
        output.action = ResetAction::Cancel;
    }
    output
}

impl crate::app::App {
    pub(crate) fn show_folder_order_reset_confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.folder_order_reset_dialog_visible() {
            return;
        }
        // A held Enter must not turn the next frame into confirmation.
        let enter = self.dialog_enter_pressed(ctx)
            && ctx.input(|input| {
                input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::Enter,
                            pressed: true,
                            repeat: false,
                            ..
                        }
                    )
                })
            });
        let escape = self.dialog_escape_pressed(ctx);
        // Inherit the OS-resolved ordinary Window theme; warning color is semantic.
        match draw_reset_confirmation(ctx, enter, escape).action {
            ResetAction::Confirm => self.resolve_folder_order_reset_confirmation(true),
            ResetAction::Cancel => self.resolve_folder_order_reset_confirmation(false),
            ResetAction::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::{NodeT, Queryable};
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn folder_order_reset_confirmation_controls_fit_small_light_dark_at_both_scales() {
        for theme in [
            crate::os_theme::ResolvedTheme::Light,
            crate::os_theme::ResolvedTheme::Dark,
        ] {
            for scale in [1.0, 2.0] {
                let geometry = Rc::new(Cell::new(None));
                let observed_geometry = Rc::clone(&geometry);
                let action = Rc::new(Cell::new(ResetAction::None));
                let observed_action = Rc::clone(&action);
                let mut harness = crate::test_ui::builder()
                    .with_context_setup(move |ctx| {
                        crate::os_theme::apply_resolved(ctx, theme);
                        crate::ui_fonts::configure_fonts(ctx);
                    })
                    .with_size(egui::vec2(1093.0, 614.0) / scale)
                    .build(move |ctx| {
                        let output = draw_reset_confirmation(ctx, false, false);
                        if output.geometry.is_some() {
                            observed_geometry.set(output.geometry);
                        }
                        if output.action != ResetAction::None {
                            observed_action.set(output.action);
                        }
                    });
                harness.run();
                let screen = harness.ctx.content_rect();
                let (window, confirm, cancel) = geometry.get().expect("shown confirmation");
                for rect in [window, confirm, cancel] {
                    assert!(
                        screen.contains_rect(rect),
                        "theme={theme:?} scale={scale} rect={rect:?}"
                    );
                }
                for (label, expected) in [
                    ("リセット", ResetAction::Confirm),
                    ("キャンセル", ResetAction::Cancel),
                ] {
                    harness.get_by_label(label).click();
                    harness.run();
                    assert_eq!(action.get(), expected);
                }
            }
        }
    }
}
