//! Rename flow for real filesystem items selected from the grid.

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;

use crate::app::App;
use crate::native_name_dialog::{NameInputRequest, NamePromptOutcome};

pub(crate) type RenameResult = crate::shell_file_ops::ShellRenameResult;
pub(crate) type RenameReceiver = mpsc::Receiver<RenameResult>;

const DIALOG_TITLE: &str = "名前の変更";

impl App {
    /// グリッドのキー操作から、単一の実ファイル / 実フォルダだけを名前変更へ渡す。
    pub(crate) fn request_grid_rename_dialog(&mut self) {
        if self.checked.len() > 1 {
            self.show_feedback_toast("名前の変更は 1 項目ずつ行ってください".to_owned());
            return;
        }
        let target_index = self.checked.iter().next().copied().or(self.selected);
        let Some(target) = target_index
            .and_then(|index| self.items.get(index))
            .and_then(crate::grid_item::GridItem::drag_source_path)
            .map(PathBuf::from)
        else {
            self.show_feedback_toast("この項目は名前を変更できません".to_owned());
            return;
        };
        self.request_rename_dialog(target);
    }

    pub(crate) fn request_rename_dialog(&mut self, target: PathBuf) {
        if self.rename_pending.is_some() {
            self.show_feedback_toast("名前の変更が完了するまでお待ちください".to_owned());
            return;
        }
        let Some(_name) = target.file_name().and_then(|name| name.to_str()) else {
            self.show_feedback_toast("この項目は名前を変更できません".to_owned());
            return;
        };
        // The request is created from a context-menu closure. Only snapshot the
        // item kind here; the synchronous native modal opens from App::update.
        self.rename_target_is_file = target.is_file();
        self.rename_target = Some(target);
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
        let Some(old_name) = target
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
        else {
            self.clear_rename_dialog_state();
            return;
        };

        let owner = self.main_hwnd;
        let target_is_file = self.rename_target_is_file;
        let caption = format!("対象: {}", target.display());
        let mut input = old_name.clone();
        // This flag is a queued request, not a flag that remains true while the
        // native dialog is open. DialogBoxIndirectParamW owns that modal span.
        self.show_rename_dialog = false;

        loop {
            let select_utf16 = target_is_file.then(|| (0, rename_stem_utf16_len(&input)));
            let request = NameInputRequest {
                owner,
                title: DIALOG_TITLE,
                caption: &caption,
                initial: &input,
                select_utf16,
            };
            let entered = match crate::native_name_dialog::prompt_name(&request) {
                NamePromptOutcome::Accepted(entered) => entered,
                NamePromptOutcome::Cancelled => {
                    self.clear_rename_dialog_state();
                    return;
                }
                NamePromptOutcome::Failed => {
                    self.clear_rename_dialog_state();
                    self.show_feedback_toast("名前の変更画面を開けませんでした".to_owned());
                    return;
                }
            };
            let name = match validate_rename_item_name(&entered) {
                Ok(name) => name,
                Err(message) => {
                    crate::native_name_dialog::show_warning(owner, DIALOG_TITLE, &message);
                    input = entered;
                    continue;
                }
            };
            if name == old_name {
                self.clear_rename_dialog_state();
                return;
            }
            if target_is_file && rename_extension_changed(&old_name, &name) {
                let message = extension_change_confirmation_message(&old_name, &name);
                if !crate::native_name_dialog::confirm_warning(owner, DIALOG_TITLE, &message) {
                    input = name;
                    continue;
                }
            }

            self.clear_rename_dialog_state();
            self.rename_pending = Some(crate::shell_file_ops::rename_item_async(
                owner, target, name,
            ));
            ctx.request_repaint();
            return;
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
                // Keep all post-rename cleanup unchanged: each path-keyed
                // subsystem must observe the same old -> new transition.
                self.release_viewer_surfaces_for_removed_paths(
                    ctx,
                    std::slice::from_ref(&outcome.target),
                    "rename_old_path",
                );
                self.migrate_video_resume_positions_for_renamed_path(
                    &outcome.target,
                    &outcome.new_path,
                );
                self.rewrite_page_edit_presence_for_rename(&outcome.target, &outcome.new_path);
                self.remove_paths_from_smart_folder_snapshots(std::slice::from_ref(
                    &outcome.target,
                ));
                self.spawn_rename_key_migration(outcome.target.clone(), outcome.new_path.clone());
                let current_matches_parent = outcome.target.parent().is_some_and(|parent| {
                    self.current_folder
                        .as_ref()
                        .is_some_and(|current| crate::folder_tree::path_eq(current, parent))
                });
                if current_matches_parent {
                    self.select_after_load = outcome
                        .new_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned);
                    self.pending_reload = true;
                }
                self.show_feedback_toast(format!(
                    "名前を変更しました: {}",
                    outcome.new_path.display()
                ));
            }
            Ok(Err(message)) => {
                self.clear_rename_dialog_state();
                crate::native_name_dialog::show_warning(self.main_hwnd, DIALOG_TITLE, &message);
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.rename_pending = Some(rx);
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.clear_rename_dialog_state();
                crate::native_name_dialog::show_warning(
                    self.main_hwnd,
                    DIALOG_TITLE,
                    "名前変更 worker が終了しました",
                );
            }
        }
    }

    fn clear_rename_dialog_state(&mut self) {
        self.show_rename_dialog = false;
        self.rename_target = None;
        self.rename_target_is_file = false;
    }
}

/// UTF-16 code units in the stem before the final extension.
/// Extensionless names and dotfiles select the whole name.
fn rename_stem_utf16_len(name: &str) -> usize {
    let Some(extension) = final_extension(name) else {
        return name.encode_utf16().count();
    };
    name[..name.len() - extension.len() - 1]
        .encode_utf16()
        .count()
}

fn final_extension(name: &str) -> Option<&str> {
    std::path::Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
}

fn rename_extension_changed(old_name: &str, new_name: &str) -> bool {
    match (final_extension(old_name), final_extension(new_name)) {
        (Some(old), Some(new)) => !old.eq_ignore_ascii_case(new),
        (None, None) => false,
        _ => true,
    }
}

fn extension_change_confirmation_message(old_name: &str, new_name: &str) -> String {
    let display = |extension: Option<&str>| match extension {
        Some(extension) => format!("\".{extension}\""),
        None => "\"(なし)\"".to_owned(),
    };
    format!(
        "拡張子が {} から {} に変わります。ファイルが開けなくなる可能性があります。変更しますか?",
        display(final_extension(old_name)),
        display(final_extension(new_name))
    )
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
    if name.chars().any(|character| {
        character.is_control()
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
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
    use super::{rename_extension_changed, rename_stem_utf16_len, validate_rename_item_name};

    #[test]
    fn rename_stem_utf16_len_handles_extensions_dotfiles_and_unicode() {
        assert_eq!(
            rename_stem_utf16_len("a.tar.gz"),
            "a.tar".encode_utf16().count()
        );
        assert_eq!(
            rename_stem_utf16_len("README"),
            "README".encode_utf16().count()
        );
        assert_eq!(
            rename_stem_utf16_len(".gitignore"),
            ".gitignore".encode_utf16().count()
        );
        assert_eq!(
            rename_stem_utf16_len("日本語画像.jpg"),
            "日本語画像".encode_utf16().count()
        );
        assert_eq!(rename_stem_utf16_len("🎬movie.mp4"), 7);
    }

    #[test]
    fn rename_extension_changed_compares_final_extension_case_insensitively() {
        assert!(!rename_extension_changed("photo.JPG", "renamed.jpg"));
        assert!(!rename_extension_changed("README", "RENAMED"));
        assert!(!rename_extension_changed(".gitignore", ".ignore"));
        assert!(rename_extension_changed("a.tar.gz", "a.tar.zip"));
        assert!(rename_extension_changed("movie.mp4", "movie"));
        assert!(rename_extension_changed("README", "README.txt"));
    }

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
