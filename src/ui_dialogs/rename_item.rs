//! Rename dialog for real filesystem items selected from the grid.

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;

use crate::app::App;

pub(crate) type RenameResult = crate::shell_file_ops::ShellRenameResult;
pub(crate) type RenameReceiver = mpsc::Receiver<RenameResult>;

impl App {
    pub(crate) fn request_rename_dialog(&mut self, target: PathBuf) {
        let Some(name) = target
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
        else {
            self.show_feedback_toast("この項目は名前を変更できません".to_owned());
            return;
        };
        self.rename_target = Some(target);
        self.rename_input = name;
        self.rename_error = None;
        self.show_rename_dialog = true;
    }

    pub(crate) fn show_rename_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.show_rename_dialog {
            return;
        }

        let Some(target) = self.rename_target.clone() else {
            self.clear_rename_dialog_state();
            return;
        };

        let pending = self.rename_pending.is_some();
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(90.0, 70.0);

        egui::Window::new("名前の変更")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                ui.label(format!("対象: {}", target.display()));
                ui.add_space(4.0);

                let resp = ui.add_enabled(
                    !pending,
                    egui::TextEdit::singleline(&mut self.rename_input).desired_width(f32::INFINITY),
                );
                if !pending && !resp.has_focus() && !ui.memory(|m| m.focused().is_some()) {
                    resp.request_focus();
                }
                if !pending && enter_pressed && (resp.has_focus() || resp.lost_focus()) {
                    apply = true;
                }

                if let Some(ref err) = self.rename_error {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(err)
                            .color(crate::ui_helpers::ERROR_TEXT_COLOR)
                            .size(crate::ui_helpers::ERROR_TEXT_SIZE),
                    );
                }

                if pending {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("変更中...");
                    });
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let can_apply = !pending && !self.rename_input.trim().is_empty();
                    if ui
                        .add_enabled(can_apply, egui::Button::new("  変更  "))
                        .clicked()
                    {
                        apply = true;
                    }
                    if ui
                        .add_enabled(!pending, egui::Button::new("キャンセル"))
                        .clicked()
                    {
                        cancel = true;
                    }
                });

                if !pending && escape_pressed {
                    cancel = true;
                }
            });

        if apply {
            match validate_rename_item_name(&self.rename_input) {
                Ok(name) => {
                    if target
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|old| old == name)
                    {
                        self.clear_rename_dialog_state();
                        return;
                    }
                    self.rename_input = name.clone();
                    self.rename_error = None;
                    self.rename_pending = Some(crate::shell_file_ops::rename_item_async(
                        self.main_hwnd,
                        target,
                        name,
                    ));
                    ctx.request_repaint();
                }
                Err(err) => {
                    self.rename_error = Some(err);
                }
            }
        } else if !pending && (cancel || !open) {
            self.clear_rename_dialog_state();
        }
    }

    pub(crate) fn poll_rename_pending(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.rename_pending.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(outcome)) => {
                self.clear_rename_dialog_state();
                if outcome.aborted {
                    self.show_feedback_toast("名前の変更をキャンセルしました".to_owned());
                    return;
                }
                let current_matches_parent = outcome.target.parent().is_some_and(|parent| {
                    self.current_favorite_target()
                        .as_ref()
                        .is_some_and(|cur| crate::folder_tree::path_eq(cur, parent))
                });
                if current_matches_parent {
                    self.select_after_load = outcome
                        .new_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(str::to_owned);
                    self.pending_reload = true;
                }
                self.show_feedback_toast(format!(
                    "名前を変更しました: {}",
                    outcome.new_path.display()
                ));
            }
            Ok(Err(err)) => {
                self.rename_error = Some(err);
                self.show_rename_dialog = true;
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.rename_pending = Some(rx);
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.rename_error = Some("名前変更 worker が終了しました".to_owned());
                self.show_rename_dialog = true;
            }
        }
    }

    fn clear_rename_dialog_state(&mut self) {
        self.show_rename_dialog = false;
        self.rename_target = None;
        self.rename_input.clear();
        self.rename_error = None;
    }
}

fn validate_rename_item_name(input: &str) -> Result<String, String> {
    let name = input.trim();
    if name.is_empty() {
        return Err("名前を入力してください".to_owned());
    }
    if name == "." || name == ".." {
        return Err("この名前は使えません".to_owned());
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err("末尾がピリオドまたは空白の名前は使えません".to_owned());
    }
    if name.chars().any(|c| {
        c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
    }) {
        return Err("名前に使えない文字が含まれています".to_owned());
    }
    if is_reserved_windows_device_name(name) {
        return Err("Windows の予約名は使えません".to_owned());
    }
    Ok(name.to_owned())
}

fn is_reserved_windows_device_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || matches_reserved_numbered_device(&stem, "COM")
        || matches_reserved_numbered_device(&stem, "LPT")
}

fn matches_reserved_numbered_device(stem: &str, prefix: &str) -> bool {
    let Some(rest) = stem.strip_prefix(prefix) else {
        return false;
    };
    rest.len() == 1 && matches!(rest.as_bytes()[0], b'1'..=b'9')
}

#[cfg(test)]
mod tests {
    use super::validate_rename_item_name;

    #[test]
    fn validate_rename_item_name_accepts_normal_names() {
        assert_eq!(
            validate_rename_item_name("  新しいフォルダー  ").unwrap(),
            "新しいフォルダー"
        );
        assert_eq!(
            validate_rename_item_name("image 01.jpg").unwrap(),
            "image 01.jpg"
        );
    }

    #[test]
    fn validate_rename_item_name_rejects_invalid_names() {
        assert!(validate_rename_item_name(r"a\b").is_err());
        assert!(validate_rename_item_name("a/b").is_err());
        assert!(validate_rename_item_name("CON").is_err());
        assert!(validate_rename_item_name("lpt9.txt").is_err());
        assert!(validate_rename_item_name("name.").is_err());
    }
}
