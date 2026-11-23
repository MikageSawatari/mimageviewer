//! Draft controls share the RGB editor and production cell painter.

use crate::folder_cell_background::FolderCellBackground;
use crate::settings::Settings;
use eframe::egui;

pub(super) fn draw_settings(ui: &mut egui::Ui, settings: &mut Settings) {
    ui.checkbox(
        &mut settings.folder_cell_background.enabled,
        "フォルダのセルの背景に色を付ける",
    );
    ui.small("選択・チェックの青い背景を優先します。OFF にしても色は保持します。画像がセルいっぱいのときは、周りの細い余白にだけ色が見えます。");
    ui.horizontal_wrapped(|ui| {
        for dark in [false, true] {
            ui.push_id(("folder_background", dark), |ui| {
                ui.label(if dark { "Dark" } else { "Light" });
                let rgb = if dark {
                    &mut settings.folder_cell_background.dark
                } else {
                    &mut settings.folder_cell_background.light
                };
                let response = super::name_colors::draw_rgb_editor(ui, rgb);
                crate::ui_helpers::set_accessibility_label(
                    &response,
                    if dark {
                        "フォルダ背景 Dark HEX"
                    } else {
                        "フォルダ背景 Light HEX"
                    },
                );
            });
        }
        if ui.button("既定の色に戻す").clicked() {
            super::name_colors::clear_hex_editor(ui.ctx());
            let enabled = settings.folder_cell_background.enabled;
            settings.folder_cell_background = FolderCellBackground {
                enabled,
                ..Default::default()
            };
        }
    });
    if !settings.folder_cell_background.valid() {
        ui.colored_label(
            ui.visuals().error_fg_color,
            "フォルダの背景は文字とのコントラスト比が4.5:1以上の色を指定してください",
        );
    }
    // Preview the retained colors even while OFF, without changing the draft.
    let preview = FolderCellBackground {
        enabled: true,
        ..settings.folder_cell_background.clone()
    };
    for dark in [false, true] {
        ui.label(if dark {
            "Dark の見本"
        } else {
            "Light の見本"
        });
        let visuals = crate::os_theme::app_visuals(
            if dark {
                crate::os_theme::ResolvedTheme::Dark
            } else {
                crate::os_theme::ResolvedTheme::Light
            },
            settings.text_contrast,
        );
        ui.scope(|ui| {
            *ui.visuals_mut() = visuals;
            crate::app::draw_folder_background_samples(ui, &preview, false);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_background_reset_preserves_enabled_and_only_edits_draft() {
        use egui_kittest::kittest::Queryable;
        let mut live = Settings::default();
        live.folder_cell_background = FolderCellBackground {
            enabled: true,
            light: [255, 255, 230],
            dark: [30, 35, 20],
        };
        let before = live.folder_cell_background.clone();
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(720.0, 520.0))
            .build_ui_state(
                |root, settings| {
                    egui::CentralPanel::default().show(root, |ui| draw_settings(ui, settings));
                },
                live.preferences_snapshot(),
            );
        harness.run();
        harness.get_by_label("既定の色に戻す").click();
        harness.run();
        assert_eq!(
            harness.state().folder_cell_background,
            FolderCellBackground {
                enabled: true,
                ..Default::default()
            }
        );
        harness
            .get_by_label("フォルダのセルの背景に色を付ける")
            .click();
        harness.run();
        assert_eq!(
            harness.state().folder_cell_background,
            FolderCellBackground::default()
        );
        assert_eq!(live.folder_cell_background, before);
    }
}
