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
        // 対象種別はダイアログを開く時に一度だけ確認し、描画中は filesystem に触れない。
        self.rename_target_is_file = target.is_file();
        self.rename_target = Some(target);
        self.rename_input = name;
        self.rename_error = None;
        self.rename_initial_selection_pending = true;
        self.rename_extension_confirm = false;
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
        let mut apply_confirmed = false;
        let mut cancel = false;
        let mut back_to_edit = false;
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

                if self.rename_extension_confirm {
                    let old_name = target
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default();
                    ui.label(extension_change_confirmation_message(
                        old_name,
                        &self.rename_input,
                    ));
                } else {
                    let resp = ui.add_enabled(
                        !pending,
                        egui::TextEdit::singleline(&mut self.rename_input)
                            .desired_width(f32::INFINITY),
                    );
                    if !pending && self.rename_initial_selection_pending {
                        resp.request_focus();
                        if let Some(mut state) =
                            egui::text_edit::TextEditState::load(ui.ctx(), resp.id)
                        {
                            let end = if self.rename_target_is_file {
                                rename_stem_char_count(&self.rename_input)
                            } else {
                                self.rename_input.chars().count()
                            };
                            state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::two(
                                    egui::text::CCursor::new(0),
                                    egui::text::CCursor::new(end),
                                )));
                            state.store(ui.ctx(), resp.id);
                        }
                        self.rename_initial_selection_pending = false;
                    } else if !pending && !resp.has_focus() && !ui.memory(|m| m.focused().is_some())
                    {
                        resp.request_focus();
                    }
                    if !pending && enter_pressed && (resp.has_focus() || resp.lost_focus()) {
                        apply = true;
                    }
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
                    if self.rename_extension_confirm {
                        if ui.button("変更する").clicked() {
                            apply_confirmed = true;
                        }
                        if ui.button("戻す").clicked() {
                            back_to_edit = true;
                        }
                    } else {
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
                    }
                });

                if !pending && escape_pressed {
                    if self.rename_extension_confirm {
                        back_to_edit = true;
                    } else {
                        cancel = true;
                    }
                }
                if self.rename_extension_confirm && enter_pressed {
                    apply_confirmed = true;
                }
            });

        if back_to_edit {
            self.rename_extension_confirm = false;
        } else if apply || apply_confirmed {
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
                    let old_name = target
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default();
                    if !apply_confirmed
                        && self.rename_target_is_file
                        && rename_extension_changed(old_name, &name)
                    {
                        self.rename_extension_confirm = true;
                        return;
                    }
                    self.rename_extension_confirm = false;
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
                // 旧 path を表示・再生中の窓を閉じる (review-v2.3.0 角度④ (A):
                // parked bundle が旧 path のまま生き残ると再生位置やメタデータ書込が
                // 旧名キーへ流れる)。
                self.release_viewer_surfaces_for_removed_paths(
                    ctx,
                    std::slice::from_ref(&outcome.target),
                    "rename_old_path",
                );
                // 旧 path キーの永続データ (★ / タグ / 回転 / 編集レイヤー / マスク /
                // ブックマーク / 続き位置 / サイドカー等) を新 path へ移行する
                // (`rename_key_migration`、worker)。in-memory 側 (再生位置 / 編集済み
                // バッジの presence set) はここで同期的に付け替える。
                self.migrate_video_resume_positions_for_renamed_path(
                    &outcome.target,
                    &outcome.new_path,
                );
                self.rewrite_page_edit_presence_for_rename(&outcome.target, &outcome.new_path);
                // 合成一覧には旧 path を残さない。新 path の採用は path-keyed メタデータ
                // 移行完了後の authoritative scan に任せる。
                self.remove_paths_from_smart_folder_snapshots(std::slice::from_ref(
                    &outcome.target,
                ));
                self.spawn_rename_key_migration(outcome.target.clone(), outcome.new_path.clone());
                // 再読み込み判定は「お気に入りへ追加できる場所」ではなく、一覧を実際に
                // 列挙した current_folder と比較する。current_favorite_target() は ZIP/PDF の
                // enumerate 中や archive_source_override が残る遷移フレームでは None を返すため、
                // 直前に ZIP を閲覧した通常フォルダで rename 成功を取りこぼし得る。
                let current_matches_parent = outcome.target.parent().is_some_and(|parent| {
                    self.current_folder
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
        self.rename_target_is_file = false;
        self.rename_initial_selection_pending = false;
        self.rename_extension_confirm = false;
    }
}

/// 最終拡張子を除いた stem の文字数。拡張子なし・dotfile は名前全体を返す。
fn rename_stem_char_count(name: &str) -> usize {
    let Some(ext) = final_extension(name) else {
        return name.chars().count();
    };
    name[..name.len() - ext.len() - 1].chars().count()
}

fn final_extension(name: &str) -> Option<&str> {
    std::path::Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
}

fn rename_extension_changed(old_name: &str, new_name: &str) -> bool {
    match (final_extension(old_name), final_extension(new_name)) {
        (Some(old), Some(new)) => !old.eq_ignore_ascii_case(new),
        (None, None) => false,
        _ => true,
    }
}

fn extension_change_confirmation_message(old_name: &str, new_name: &str) -> String {
    let display = |ext: Option<&str>| match ext {
        Some(ext) => format!("\".{ext}\""),
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
    use super::{rename_extension_changed, rename_stem_char_count, validate_rename_item_name};

    #[test]
    fn rename_stem_char_count_handles_extensions_dotfiles_and_unicode() {
        assert_eq!(rename_stem_char_count("a.tar.gz"), "a.tar".chars().count());
        assert_eq!(rename_stem_char_count("README"), "README".chars().count());
        assert_eq!(
            rename_stem_char_count(".gitignore"),
            ".gitignore".chars().count()
        );
        assert_eq!(
            rename_stem_char_count("日本語画像.jpg"),
            "日本語画像".chars().count()
        );
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
