//! Shared model-addon controls. Draw only cached owner state here.
use crate::addon::InstallProgress;
use eframe::egui;

pub(super) fn draw_actions(ui: &mut egui::Ui, busy: bool, can_remove: bool) -> (bool, bool, bool) {
    let mut result = (false, false, false);
    ui.horizontal_wrapped(|ui| {
        result.0 = ui
            .add_enabled(!busy, egui::Button::new("ダウンロード"))
            .clicked();
        result.1 = ui
            .add_enabled(can_remove, egui::Button::new("削除"))
            .clicked();
        result.2 = ui.button("フォルダを開く").clicked();
    });
    result
}

pub(super) fn draw_progress(ui: &mut egui::Ui, progress: &InstallProgress) {
    match progress {
        InstallProgress::Downloading {
            bytes_done,
            bytes_total,
        } => {
            let ratio = if *bytes_total > 0 {
                *bytes_done as f32 / *bytes_total as f32
            } else {
                0.0
            };
            ui.add(egui::ProgressBar::new(ratio.clamp(0.0, 1.0)).text(format!(
                "{:.2} / {:.2} GB",
                *bytes_done as f64 / 1e9,
                *bytes_total as f64 / 1e9
            )));
        }
        InstallProgress::Idle | InstallProgress::FetchingIndex => {
            ui.label("配布一覧を取得中...");
        }
        InstallProgress::Verifying => {
            ui.label("ファイルを検証中...");
        }
        InstallProgress::Concatenating => {
            ui.label("モデルのファイルをまとめています...");
        }
        InstallProgress::Installing => {
            ui.label("モデルを配置中...");
        }
    }
}
