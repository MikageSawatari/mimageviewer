//! `show_open_folder_dialog_window` ダイアログの実装。
//!
//! アドレスバーを非表示にしている場合でも、フォルダを直接入力して
//! 開けるようにするためのモーダルダイアログ。「ファイル → フォルダを開く…」
//! メニューから呼ばれる。
//!
//! `update()` から `self.show_open_folder_dialog_window(ctx)` で呼ばれ、
//! 決定されたパスを返す (呼び出し側で `load_folder` へ渡す)。

#![allow(unused_imports)]

use std::path::PathBuf;

use eframe::egui;

use crate::app::App;

impl App {
    pub(crate) fn show_open_folder_dialog_window(
        &mut self,
        ctx: &egui::Context,
    ) -> Option<PathBuf> {
        if !self.show_open_folder_dialog {
            return None;
        }

        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let dialog_pos = ctx.content_rect().min + egui::vec2(80.0, 60.0);
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);

        egui::Window::new("フォルダを開く")
            .drag_area(egui::WindowDrag::Anywhere)
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(520.0);
                ui.label("開きたいフォルダのパスを入力してください:");
                ui.add_space(4.0);

                let mut output = crate::ime_focus::show_singleline(
                    ui,
                    &mut self.open_folder_input,
                    None,
                    |edit| {
                        edit.desired_width(f32::INFINITY)
                            .hint_text(r"例: C:\Users\you\Pictures")
                    },
                );
                crate::ui_helpers::singleline_text_edit_context_menu(
                    ui,
                    &mut output,
                    &mut self.open_folder_input,
                );
                let resp = output.response;
                // 初回フォーカス
                if !resp.has_focus() && !ui.memory(|m| m.focused().is_some()) {
                    resp.request_focus();
                }
                // Enter で決定（フォーカス中 or フォーカスを失った瞬間）
                if enter_pressed && (resp.has_focus() || resp.lost_focus()) {
                    apply = true;
                }

                // エラーメッセージ
                if let Some(ref err) = self.open_folder_error {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(err)
                            .color(crate::ui_helpers::ERROR_TEXT_COLOR)
                            .size(crate::ui_helpers::ERROR_TEXT_SIZE),
                    );
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let can_apply = !self.open_folder_input.trim().is_empty();
                    if ui
                        .add_enabled(can_apply, egui::Button::new("  開く  "))
                        .clicked()
                    {
                        apply = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
                // Esc でキャンセル (IME 変換中は dialog_escape_pressed が false を返す)
                if escape_pressed {
                    cancel = true;
                }
            });

        let mut result: Option<PathBuf> = None;
        if apply {
            let input = self.open_folder_input.trim().to_string();
            if !input.is_empty() {
                let p = crate::path_key::normalize_input_path(std::path::Path::new(&input));
                if p.exists() {
                    if let Some(resolved) = crate::folder_tree::resolve_openable_path(&p) {
                        result = Some(resolved);
                        self.show_open_folder_dialog = false;
                        self.open_folder_input.clear();
                        self.open_folder_error = None;
                    } else {
                        self.open_folder_error =
                            Some(format!("開けるフォルダが見つかりません: {input}"));
                    }
                } else {
                    self.open_folder_error = Some(format!("パスが存在しません: {input}"));
                }
            }
        } else if cancel || !open {
            self.show_open_folder_dialog = false;
            self.open_folder_input.clear();
            self.open_folder_error = None;
        }

        result
    }
}

#[cfg(test)]
mod ime_handler_tests {
    use super::*;
    use egui_kittest::kittest::{NodeT, Queryable};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    #[derive(Clone, Copy, Debug)]
    enum Dialog {
        Tags,
        OpenFolder,
        Password,
    }

    fn run_committed_dialog_key(dialog: Dialog, key: egui::Key, same_pass: bool) {
        let mut env = crate::app::setup_app_for_test();
        let target = env.tmp.path().join("committed-folder");
        std::fs::create_dir_all(&target).unwrap();
        let committed = match dialog {
            Dialog::Tags => {
                env.settings.tags.clear();
                env.open_tag_editor();
                env.tag_editor_draft
                    .push(crate::settings::TagDef::new(String::new()));
                "committed-tag".to_owned()
            }
            Dialog::OpenFolder => {
                env.show_open_folder_dialog = true;
                env.open_folder_input.clear();
                target.to_string_lossy().to_string()
            }
            Dialog::Password => {
                let pdf = env.tmp.path().join("password.pdf");
                std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
                env.pdf_password_request = Some(crate::app::PdfPasswordRequest::legacy(pdf));
                env.pdf_password_input.clear();
                "committed-password".to_owned()
            }
        };
        let app = Rc::new(RefCell::new(env));
        let render = Rc::clone(&app);
        let opened = Rc::new(RefCell::new(None));
        let opened_render = Rc::clone(&opened);
        let fonts = Cell::new(false);
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(1000.0, 600.0))
            .build_ui(move |root_ui| {
                let ctx = &root_ui.ctx().clone();
                crate::ime_focus::install_ime_input_policy(ctx);
                if !fonts.replace(true) {
                    crate::ui_fonts::configure_fonts(ctx);
                    ctx.request_repaint();
                    return;
                }
                let mut app = render.borrow_mut();
                app.update_ime_state(ctx);
                match dialog {
                    Dialog::Tags => app.show_tag_editor_dialog(ctx),
                    Dialog::OpenFolder => {
                        if let Some(path) = app.show_open_folder_dialog_window(ctx) {
                            *opened_render.borrow_mut() = Some(path);
                        }
                    }
                    Dialog::Password => app.show_pdf_password_dialog_window(ctx),
                }
            });
        harness.run();
        harness
            .get_by_role(match dialog {
                Dialog::Password => egui::accesskit::Role::PasswordInput,
                _ => egui::accesskit::Role::TextInput,
            })
            .click();
        harness.run();
        assert!(harness.ctx.memory(|memory| memory.focused().is_some()));
        harness.input_mut().events = vec![
            egui::Event::Ime(egui::ImeEvent::Enabled),
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "preedit".into(),
                active_range_chars: None,
            }),
        ];
        harness.step();
        assert!(app.borrow().ime_input_active(&harness.ctx));
        assert!(!app.borrow().dialog_enter_pressed(&harness.ctx));
        assert!(!app.borrow().dialog_escape_pressed(&harness.ctx));
        let mut events = vec![
            egui::Event::Ime(egui::ImeEvent::Commit(committed.clone())),
            egui::Event::Ime(egui::ImeEvent::Disabled),
        ];
        if !same_pass {
            harness.input_mut().events = std::mem::take(&mut events);
            harness.step();
        }
        events.push(egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        // Feed Commit + Disabled + the ordinary key to one actual handler pass.
        harness.input_mut().events = events;
        harness.step();
        assert!(!app.borrow().ime_input_active(&harness.ctx));
        let app = app.borrow();
        match dialog {
            Dialog::Tags => {
                assert!(
                    !app.show_tag_editor,
                    "{dialog:?}, {key:?}, same_pass={same_pass}"
                );
                if key == egui::Key::Enter {
                    assert_eq!(app.settings.tags.len(), 1);
                    assert_eq!(app.settings.tags[0].name, committed);
                } else {
                    assert!(app.settings.tags.is_empty());
                    assert!(app.tag_editor_draft.is_empty());
                }
                assert!(
                    app.tag_maintenance_rx.is_none(),
                    "new tags need no retag worker"
                );
            }
            Dialog::OpenFolder => {
                assert!(
                    !app.show_open_folder_dialog,
                    "{dialog:?}, {key:?}, same_pass={same_pass}"
                );
                assert_eq!(
                    *opened.borrow(),
                    (key == egui::Key::Enter).then_some(target)
                );
                assert!(app.open_folder_input.is_empty());
                assert!(app.open_folder_error.is_none());
            }
            Dialog::Password => {
                assert!(
                    app.pdf_password_dialog_path().is_none(),
                    "{dialog:?}, {key:?}, same_pass={same_pass}"
                );
                if key == egui::Key::Enter {
                    assert_eq!(
                        app.pdf_current_password.as_deref(),
                        Some(committed.as_str())
                    );
                } else {
                    assert!(app.pdf_current_password.is_none());
                }
                assert!(app.pdf_password_input.is_empty());
                assert!(app.pdf_password_error.is_none());
            }
        }
    }

    #[test]
    fn tag_editor_ime_commit_immediate_enter_applies_same_and_next_pass() {
        for same_pass in [true, false] {
            run_committed_dialog_key(Dialog::Tags, egui::Key::Enter, same_pass);
        }
    }

    #[test]
    fn tag_editor_ime_commit_immediate_escape_cancels_same_and_next_pass() {
        for same_pass in [true, false] {
            run_committed_dialog_key(Dialog::Tags, egui::Key::Escape, same_pass);
        }
    }

    #[test]
    fn open_folder_ime_commit_immediate_enter_opens_same_and_next_pass() {
        for same_pass in [true, false] {
            run_committed_dialog_key(Dialog::OpenFolder, egui::Key::Enter, same_pass);
        }
    }

    #[test]
    fn open_folder_ime_commit_immediate_escape_cancels_same_and_next_pass() {
        for same_pass in [true, false] {
            run_committed_dialog_key(Dialog::OpenFolder, egui::Key::Escape, same_pass);
        }
    }

    #[test]
    fn pdf_password_ime_commit_immediate_enter_retries_same_and_next_pass() {
        for same_pass in [true, false] {
            run_committed_dialog_key(Dialog::Password, egui::Key::Enter, same_pass);
        }
    }

    #[test]
    fn pdf_password_ime_commit_immediate_escape_cancels_same_and_next_pass() {
        for same_pass in [true, false] {
            run_committed_dialog_key(Dialog::Password, egui::Key::Escape, same_pass);
        }
    }
}
