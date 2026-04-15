//! PDF パスワード入力ダイアログ。
//!
//! パスワード付き PDF を開くときに表示され、パスワード入力と
//! DPAPI 暗号化による保存オプションを提供する。

impl crate::app::App {
    /// PDF パスワード入力ダイアログを表示する。
    ///
    /// パスワードが入力され OK されると、PDF を再読み込みする。
    pub(crate) fn show_pdf_password_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.show_pdf_password_dialog {
            return;
        }

        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let dialog_pos = ctx.content_rect().min + egui::vec2(80.0, 60.0);
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);

        egui::Window::new("PDF パスワード入力")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(380.0);
                ui.label("このPDFファイルを開くにはパスワードが必要です:");

                if let Some(ref path) = self.pdf_password_pending_path {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("?");
                    ui.label(
                        egui::RichText::new(name)
                            .size(12.0)
                            .color(egui::Color32::from_gray(120)),
                    );
                }
                ui.add_space(4.0);

                // パスワード入力欄
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.pdf_password_input)
                        .password(true)
                        .desired_width(f32::INFINITY)
                        .hint_text("パスワード"),
                );
                // 初回フォーカス
                if !resp.has_focus() && !ui.memory(|m| m.focused().is_some()) {
                    resp.request_focus();
                }
                // Enter キーで確定
                if enter_pressed && (resp.has_focus() || resp.lost_focus()) {
                    apply = true;
                }

                // エラーメッセージ
                if let Some(ref err) = self.pdf_password_error {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(err)
                            .color(crate::ui_helpers::ERROR_TEXT_COLOR)
                            .size(crate::ui_helpers::ERROR_TEXT_SIZE),
                    );
                }

                ui.add_space(4.0);

                // パスワード保存チェックボックス
                ui.checkbox(
                    &mut self.pdf_password_save,
                    "パスワードを保存する (DPAPI 暗号化)",
                );

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                // ボタン
                ui.horizontal(|ui| {
                    let can_apply = !self.pdf_password_input.trim().is_empty();
                    if ui
                        .add_enabled(can_apply, egui::Button::new("  OK  "))
                        .clicked()
                    {
                        apply = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
                if escape_pressed {
                    cancel = true;
                }
            });

        if apply {
            let password = self.pdf_password_input.trim().to_string();
            if !password.is_empty() {
                if let Some(pending_path) = self.pdf_password_pending_path.take() {
                    // パスワードが正しいか検証
                    match crate::pdf_loader::enumerate_pages(&pending_path, Some(&password)) {
                        Ok(_) => {
                            // 成功 → パスワード保存 & PDF を開く
                            if self.pdf_password_save {
                                self.pdf_passwords.set(&pending_path, &password);
                                self.pdf_passwords.save();
                            }
                            self.pdf_current_password = Some(password);
                            self.show_pdf_password_dialog = false;
                            self.pdf_password_input.clear();
                            self.pdf_password_error = None;
                            self.load_pdf_as_folder(pending_path);
                        }
                        Err(_) => {
                            // 失敗 → エラー表示してダイアログを維持
                            self.pdf_password_error =
                                Some("パスワードが正しくありません".to_string());
                            self.pdf_password_pending_path = Some(pending_path);
                        }
                    }
                }
            }
        } else if cancel || !open {
            self.show_pdf_password_dialog = false;
            self.pdf_password_input.clear();
            self.pdf_password_error = None;
            self.pdf_password_pending_path = None;
        }
    }
}
