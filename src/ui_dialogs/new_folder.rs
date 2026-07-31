//! New folder flow started from the grid background context menu.

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;

use crate::app::App;
use crate::native_name_dialog::{NameInputRequest, NamePromptOutcome};

const DIALOG_TITLE: &str = "新しいフォルダ";
const DEFAULT_NEW_FOLDER_NAME: &str = "新しいフォルダー";

pub(crate) type NewFolderResult = Result<NewFolderCreated, String>;
pub(crate) type NewFolderReceiver = mpsc::Receiver<NewFolderResult>;

pub(crate) struct NewFolderCreated {
    pub parent: PathBuf,
    pub path: PathBuf,
}

impl App {
    pub(crate) fn request_new_folder_dialog(&mut self, parent: PathBuf) {
        if self.new_folder_pending.is_some() {
            self.show_feedback_toast("フォルダ作成が完了するまでお待ちください".to_owned());
            return;
        }
        self.new_folder_parent = Some(parent);
        self.show_new_folder_dialog = true;
    }

    pub(crate) fn show_new_folder_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.show_new_folder_dialog {
            return;
        }
        let Some(parent) = self.new_folder_parent.clone() else {
            self.clear_new_folder_dialog_state();
            return;
        };

        let owner = self.main_hwnd;
        let caption = format!("作成先: {}", parent.display());
        let mut input = DEFAULT_NEW_FOLDER_NAME.to_owned();
        // This flag queues one native modal launch. No egui window remains
        // visible while the filesystem worker runs.
        self.show_new_folder_dialog = false;

        loop {
            let request = NameInputRequest {
                owner,
                title: DIALOG_TITLE,
                caption: &caption,
                initial: &input,
                select_utf16: None,
            };
            let entered = match crate::native_name_dialog::prompt_name(&request) {
                NamePromptOutcome::Accepted(entered) => entered,
                NamePromptOutcome::Cancelled => {
                    self.clear_new_folder_dialog_state();
                    return;
                }
                NamePromptOutcome::Failed => {
                    self.clear_new_folder_dialog_state();
                    self.show_feedback_toast("フォルダ作成画面を開けませんでした".to_owned());
                    return;
                }
            };
            match validate_new_folder_name(&entered) {
                Ok(name) => {
                    self.clear_new_folder_dialog_state();
                    self.new_folder_pending = Some(spawn_new_folder(parent, name));
                    ctx.request_repaint();
                    return;
                }
                Err(message) => {
                    crate::native_name_dialog::show_warning(owner, DIALOG_TITLE, &message);
                    input = entered;
                }
            }
        }
    }

    pub(crate) fn poll_new_folder_pending(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.new_folder_pending.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(created)) => {
                self.clear_new_folder_dialog_state();
                let current_matches_parent = self
                    .current_favorite_target()
                    .as_ref()
                    .is_some_and(|current| crate::folder_tree::path_eq(current, &created.parent));
                if current_matches_parent {
                    self.select_after_load = created
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned);
                    self.pending_reload = true;
                }
                self.show_feedback_toast(format!(
                    "フォルダを作成しました: {}",
                    created.path.display()
                ));
            }
            Ok(Err(message)) => {
                self.clear_new_folder_dialog_state();
                crate::native_name_dialog::show_warning(self.main_hwnd, DIALOG_TITLE, &message);
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.new_folder_pending = Some(rx);
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.clear_new_folder_dialog_state();
                crate::native_name_dialog::show_warning(
                    self.main_hwnd,
                    DIALOG_TITLE,
                    "フォルダ作成 worker が終了しました",
                );
            }
        }
    }

    fn clear_new_folder_dialog_state(&mut self) {
        self.show_new_folder_dialog = false;
        self.new_folder_parent = None;
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
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        format!("同名のファイルまたはフォルダが既にあります: {name}")
                    } else {
                        format!("フォルダを作成できません: {error}")
                    }
                });
            let _ = tx.send(result);
        });
    if let Err(error) = spawn_result {
        let _ = tx_on_spawn_error.send(Err(format!(
            "フォルダ作成 worker を開始できません: {error}"
        )));
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
    if name.chars().any(|character| {
        character.is_control()
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
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
