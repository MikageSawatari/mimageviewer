use eframe::egui;

use crate::app::App;
use crate::settings::{AiFeatureMode, UiTheme};

impl App {
    pub(crate) fn show_first_setup_dialog(&mut self, ctx: &egui::Context) {
        // settings.db を読めず defaults で保護起動した場合は「初回」ではない。
        // 復元案内を優先し、既定値を初回設定として保存しようとする誤解を防ぐ。
        if self.settings_boot_problem_source.is_some() || self.settings.first_setup_completed {
            return;
        }

        let enter_pressed = self.dialog_enter_pressed(ctx);
        let response = draw_first_setup_dialog(ctx, &mut self.settings);
        let completed =
            first_setup_should_confirm(&response, enter_pressed, self.ime_input_active(ctx));

        if completed {
            self.settings.first_setup_completed = true;
            self.settings.save();
            self.apply_ai_feature_mode_change();
        }
    }
}

fn first_setup_should_confirm(
    response: &egui::ModalResponse<egui::Response>,
    enter_pressed: bool,
    ime_active: bool,
) -> bool {
    // Enter on a focused option/link belongs to that widget.
    let focus_allows_confirm = response
        .inner
        .ctx
        .memory(|memory| memory.focused().is_none_or(|id| id == response.inner.id));
    // egui also synthesizes button clicks from focused Enter/Space. Do not let
    // that bypass the dialog IME policy; keep pointer/accessibility activation.
    let ime_keyboard_click = ime_active
        && response.inner.ctx.input(|input| {
            input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
        });
    response.inner.clicked_by(egui::PointerButton::Primary)
        || (response.inner.clicked() && !ime_keyboard_click)
        || (response.is_top_modal
            && !response.any_popup_open
            && focus_allows_confirm
            && enter_pressed)
}

/// Pure dialog rendering shared with small-viewport regression and snapshot tests.
/// Escape/backdrop clicks deliberately do not complete or skip first setup.
#[doc(hidden)]
pub fn draw_first_setup_dialog(
    ctx: &egui::Context,
    settings: &mut crate::settings::Settings,
) -> egui::ModalResponse<egui::Response> {
    egui::Modal::new(egui::Id::new("first_setup_modal")).show(ctx, |ui| {
        ui.set_width(560.0_f32.min((ctx.content_rect().width() - 48.0).max(1.0)));
        ui.heading("初回設定");
        ui.add_space(8.0);
        ui.label("使い始める前に、表示とAI処理の基本設定を選んでください。");

        let footer = super::startup_dialog_footer_height(ui, &["開始"], 26.0);
        super::startup_dialog_scroll_body(ui, "first_setup_options", footer, |ui| {
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
            ui.label(egui::RichText::new("テーマ").strong());
            if settings.ui_theme == UiTheme::Standard {
                settings.ui_theme = UiTheme::Light;
            }
            ui.radio_value(&mut settings.ui_theme, UiTheme::System, "システムと同じ");
            ui.radio_value(&mut settings.ui_theme, UiTheme::Light, "ライト");
            ui.radio_value(&mut settings.ui_theme, UiTheme::Dark, "ダーク");

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("表示時の AI 処理 (アップスケール / ノイズ除去)").strong(),
            );
            for &mode in AiFeatureMode::all() {
                ui.radio_value(
                    &mut settings.ai_feature_mode,
                    mode,
                    format!("{} - {}", mode.label(), mode.description()),
                );
            }
            ui.label(
                egui::RichText::new(
                    "画像を見るときの自動アップスケール / ノイズ除去だけを切り替えます。\n\
                     消しゴムや補正の被写体マスクなど編集ツールの AI は影響を受けません。\n\
                     AI 処理は GPU 負荷が高いため、表示が重い環境では「なし」を推奨します。\n\
                     高画質には、GPU によっては処理に時間がかかる AI モデルが含まれます。",
                )
                .weak(),
            );
            if ui
                .link("処理時間の目安を開く")
                .on_hover_text("ブラウザでマニュアルの AI 処理時間表を開きます。")
                .clicked()
            {
                let url =
                    crate::ui_helpers::manual_url("settings.html", Some("ai-processing-time"));
                crate::ui_helpers::open_url(&url);
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
            ui.label(egui::RichText::new("ビューワモード").strong());
            ui.radio_value(
                &mut settings.detached_viewer_open_images_in_window,
                false,
                "フル機能ウィンドウ（編集機能あり）",
            );
            // サブ選択肢 (本の開き方 / 画像フォルダ) は ui.indent で 1 段字下げして、
            // 上位のモード選択 (フル機能 / 複数ウィンドウ) との階層を視覚的に分ける。
            // 字下げが無いと 4 つの radio が同列に見えて何を選ぶ設定か分からなくなる
            // (環境設定ページ pages.rs の viewer_mode_* と同じパターン)。
            ui.indent("first_setup_viewer_mode_full", |ui| {
                ui.add_enabled_ui(!settings.detached_viewer_open_images_in_window, |ui| {
                    ui.radio_value(
                        &mut settings.auto_fullscreen_zip_pdf,
                        false,
                        "本はページ一覧を表示して開く",
                    );
                    ui.radio_value(
                        &mut settings.auto_fullscreen_zip_pdf,
                        true,
                        "本はページを表示して開く",
                    );
                    ui.add_enabled_ui(settings.auto_fullscreen_zip_pdf, |ui| {
                        ui.checkbox(
                            &mut settings.auto_fullscreen_image_folders,
                            "画像のみのフォルダは、PDF/ZIP のように本として扱う",
                        );
                    });
                });
            });
            ui.add_space(8.0);
            ui.radio_value(
                &mut settings.detached_viewer_open_images_in_window,
                true,
                "複数ウィンドウ（編集機能なし）",
            );
            ui.indent("first_setup_viewer_mode_multi", |ui| {
                ui.add_enabled_ui(settings.detached_viewer_open_images_in_window, |ui| {
                    ui.checkbox(
                        &mut settings.auto_fullscreen_image_folders,
                        "画像のみのフォルダは、PDF/ZIP のように本として扱う",
                    );
                });
            });
        });
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(6.0);
        ui.button("開始")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn first_setup_start_button_is_visible_and_clickable_on_small_viewports() {
        for size in [
            egui::vec2(1093.0, 614.0),
            egui::vec2(1366.0, 728.0),
            egui::vec2(640.0, 480.0),
            egui::vec2(480.0, 360.0),
        ] {
            let mut fonts_ready = false;
            let mut harness = Harness::builder().with_size(size).build_state(
                move |ctx,
                      state: &mut (
                    crate::settings::Settings,
                    bool,
                    Option<(egui::Rect, egui::Rect)>,
                )| {
                    if !fonts_ready {
                        crate::ui_fonts::configure_fonts(ctx);
                        fonts_ready = true;
                        ctx.request_repaint();
                        return;
                    }
                    let response = draw_first_setup_dialog(ctx, &mut state.0);
                    state.2 = Some((response.inner.rect, response.inner.interact_rect));
                    state.1 |= first_setup_should_confirm(&response, false, false);
                },
                (crate::settings::Settings::default(), false, None),
            );
            harness.run();
            let (rect, interact_rect) = harness.state().2.unwrap();
            assert!(
                egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(rect),
                "{size:?}: {rect:?}"
            );
            assert!(
                interact_rect.contains_rect(rect),
                "{size:?}: {rect:?} clipped to {interact_rect:?}"
            );
            harness.get_by_label("開始").click();
            harness.run();
            assert!(harness.state().1, "{size:?}");
        }
    }

    #[test]
    fn first_setup_enter_confirms_defaults_but_ime_enter_and_escape_do_not() {
        for (key, composing, focus_start, expected) in [
            (egui::Key::Enter, false, false, true),
            (egui::Key::Enter, true, false, false),
            (egui::Key::Escape, false, false, false),
            (egui::Key::Enter, false, true, true),
            (egui::Key::Enter, true, true, false),
            (egui::Key::Space, true, true, false),
        ] {
            // Test the handler's decision without saving settings or starting AI.
            let mut app = crate::app::setup_app_for_test();
            app.settings.first_setup_completed = false;
            let ctx = egui::Context::default();
            crate::ime_focus::install_ime_input_policy(&ctx);
            // Modal ownership and Area geometry are published on the next frame.
            let mut start_id = None;
            for _ in 0..3 {
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    start_id = Some(draw_first_setup_dialog(ctx, &mut app.settings).inner.id);
                });
            }
            if focus_start {
                ctx.memory_mut(|memory| memory.request_focus(start_id.unwrap()));
            }
            let mut events = Vec::new();
            if composing {
                events.push(egui::Event::Ime(egui::ImeEvent::Enabled));
                events.push(egui::Event::Ime(egui::ImeEvent::Preedit("設定".into())));
            }
            events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    let enter = app.dialog_enter_pressed(ctx);
                    let response = draw_first_setup_dialog(ctx, &mut app.settings);
                    let ime_active = app.ime_input_active(ctx);
                    assert_eq!(
                        first_setup_should_confirm(&response, enter, ime_active),
                        expected
                    );
                    if expected && !focus_start {
                        ctx.memory_mut(|memory| {
                            memory.request_focus(egui::Id::new("focused_option"))
                        });
                        assert!(!first_setup_should_confirm(&response, enter, ime_active));
                    }
                },
            );
            assert!(!app.settings.first_setup_completed);
        }
    }
}
