//! New folder dialog shown from the grid background context menu.

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;

use crate::app::App;

const DEFAULT_NEW_FOLDER_NAME: &str = "新しいフォルダー";

pub(crate) type NewFolderResult = Result<NewFolderCreated, String>;
pub(crate) type NewFolderReceiver = mpsc::Receiver<NewFolderResult>;

pub(crate) struct NewFolderCreated {
    pub parent: PathBuf,
    pub path: PathBuf,
}

impl App {
    pub(crate) fn request_new_folder_dialog(&mut self, parent: PathBuf) {
        self.new_folder_parent = Some(parent);
        self.new_folder_input = DEFAULT_NEW_FOLDER_NAME.to_owned();
        self.new_folder_error = None;
        self.show_new_folder_dialog = true;
    }

    pub(crate) fn show_new_folder_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.show_new_folder_dialog {
            return;
        }

        let Some(parent) = self.new_folder_parent.clone() else {
            self.show_new_folder_dialog = false;
            self.new_folder_input.clear();
            self.new_folder_error = None;
            return;
        };

        let pending = self.new_folder_pending.is_some();
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(90.0, 70.0);

        egui::Window::new("新しいフォルダ")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                ui.label(format!("作成先: {}", parent.display()));
                ui.add_space(4.0);

                let resp = ui.add_enabled(
                    !pending,
                    egui::TextEdit::singleline(&mut self.new_folder_input)
                        .desired_width(f32::INFINITY)
                        .hint_text(DEFAULT_NEW_FOLDER_NAME),
                );
                if !pending && !resp.has_focus() && !ui.memory(|m| m.focused().is_some()) {
                    resp.request_focus();
                }
                if !pending && enter_pressed && (resp.has_focus() || resp.lost_focus()) {
                    apply = true;
                }

                if let Some(ref err) = self.new_folder_error {
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
                        ui.label("作成中…");
                    });
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let can_apply = !pending && !self.new_folder_input.trim().is_empty();
                    if ui
                        .add_enabled(can_apply, egui::Button::new("  作成  "))
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
            match validate_new_folder_name(&self.new_folder_input) {
                Ok(name) => {
                    self.new_folder_input = name.clone();
                    self.new_folder_error = None;
                    self.new_folder_pending = Some(spawn_new_folder(parent, name));
                    ctx.request_repaint();
                }
                Err(err) => {
                    self.new_folder_error = Some(err);
                }
            }
        } else if !pending && (cancel || !open) {
            self.show_new_folder_dialog = false;
            self.new_folder_parent = None;
            self.new_folder_input.clear();
            self.new_folder_error = None;
        }
    }

    pub(crate) fn poll_new_folder_pending(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.new_folder_pending.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(created)) => {
                self.show_new_folder_dialog = false;
                self.new_folder_parent = None;
                self.new_folder_input.clear();
                self.new_folder_error = None;
                let current_matches_parent = self
                    .current_favorite_target()
                    .as_ref()
                    .is_some_and(|cur| crate::folder_tree::path_eq(cur, &created.parent));
                if current_matches_parent {
                    self.select_after_load = created
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(str::to_owned);
                    self.pending_reload = true;
                }
                self.show_feedback_toast(format!(
                    "フォルダを作成しました: {}",
                    created.path.display()
                ));
            }
            Ok(Err(err)) => {
                self.new_folder_error = Some(err);
                self.show_new_folder_dialog = true;
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.new_folder_pending = Some(rx);
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.new_folder_error = Some("フォルダ作成 worker が終了しました".to_owned());
                self.show_new_folder_dialog = true;
            }
        }
    }
}

fn spawn_new_folder(parent: PathBuf, name: String) -> NewFolderReceiver {
    let (tx, rx) = mpsc::channel();
    let tx_on_spawn_error = tx.clone();
    let spawn_result = std::thread::Builder::new()
        .name("new-folder-worker".into())
        .spawn(move || {
            let path = parent.join(&name);
            let result = std::fs::create_dir(&path)
                .map(|()| NewFolderCreated { parent, path })
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        format!("同名のファイルまたはフォルダが既にあります: {name}")
                    } else {
                        format!("フォルダを作成できません: {e}")
                    }
                });
            let _ = tx.send(result);
        });
    if let Err(e) = spawn_result {
        let _ = tx_on_spawn_error.send(Err(format!("フォルダ作成 worker を開始できません: {e}")));
    }
    rx
}

fn validate_new_folder_name(input: &str) -> Result<String, String> {
    let name = input.trim();
    if name.is_empty() {
        return Err("フォルダ名を入力してください".to_owned());
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
        return Err("フォルダ名に使えない文字が含まれています".to_owned());
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
    use super::validate_new_folder_name;

    #[test]
    fn validate_new_folder_name_accepts_normal_names() {
        assert_eq!(
            validate_new_folder_name("  新しいフォルダー  ").unwrap(),
            "新しいフォルダー"
        );
        assert_eq!(validate_new_folder_name("album 01").unwrap(), "album 01");
    }

    #[test]
    fn validate_new_folder_name_rejects_path_separators_and_reserved_names() {
        assert!(validate_new_folder_name(r"a\b").is_err());
        assert!(validate_new_folder_name("a/b").is_err());
        assert!(validate_new_folder_name("CON").is_err());
        assert!(validate_new_folder_name("com1.txt").is_err());
        assert!(validate_new_folder_name("name.").is_err());
    }
}
