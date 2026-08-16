use std::path::Path;

use crate::app::App;
use eframe::egui;

#[derive(Default)]
pub struct NetworkDataDirNoticeResponse {
    pub open_manual: bool,
    pub close: bool,
    pub dismiss_for_path: bool,
}

/// 案内の順序は「まず本体をこの PC へ」→「どうしても置いたままにするなら別の手がある」。
/// `--data-dir` を先に出すと、多くの利用者はそこで止まる。引数の指定はマニュアルへ送る。
pub fn render_network_data_dir_notice_content(
    ui: &mut egui::Ui,
    data_dir: &Path,
) -> NetworkDataDirNoticeResponse {
    let mut response = NetworkDataDirNoticeResponse::default();

    ui.label("mImageViewer が使うデータの保存先がネットワーク上にあります。");
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(data_dir.display().to_string())
            .monospace()
            .strong(),
    );
    ui.add_space(8.0);
    ui.label(
        "この置き方はサポートしていません。サムネイルや検索用の索引などをこの場所に保存すると、\
         起動や表示が遅くなったり、終了後の再起動で応答しなくなったりすることがあります。",
    );
    ui.add_space(8.0);
    ui.colored_label(
        ui.visuals().warn_fg_color,
        "同じ保存先を複数の PC から使わないでください。データが壊れることがあり、\
         mImageViewer 側では防げません。",
    );
    ui.add_space(10.0);
    ui.label(egui::RichText::new("おすすめ").strong());
    ui.add_space(2.0);
    ui.label(
        "mImageViewer 本体を、この PC のディスクに置いてください。\
         画像はネットワーク上のままで構いません。",
    );
    ui.add_space(8.0);
    ui.label(
        "本体をネットワーク上に置いたままにしたい場合は、\
         mImageViewer が使うデータだけをこの PC に置く方法もあります。",
    );
    ui.add_space(4.0);
    if ui.link("詳しい手順をマニュアルで見る").clicked() {
        response.open_manual = true;
    }
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button("閉じる").clicked() {
            response.close = true;
        }
        if ui.button("この保存先では今後表示しない").clicked() {
            response.dismiss_for_path = true;
        }
    });

    response
}

impl App {
    pub(crate) fn network_data_dir_notice_visible(&self) -> bool {
        self.network_data_dir_notice.is_some()
            && self.settings_boot_problem_source.is_none()
            && self.settings.first_setup_completed
            && !self.show_mouse_nav_migration_prompt
            && !self.show_whats_new
    }

    pub(crate) fn show_network_data_dir_notice_dialog(&mut self, ctx: &egui::Context) {
        if !self.network_data_dir_notice_visible() {
            return;
        }
        let Some(data_dir) = self.network_data_dir_notice.clone() else {
            return;
        };

        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
        let mut response = NetworkDataDirNoticeResponse::default();
        egui::Window::new("データの保存先について")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(dialog_pos)
            .min_width(460.0)
            .show(ctx, |ui| {
                ui.set_max_width(560.0);
                response = render_network_data_dir_notice_content(ui, &data_dir);
            });

        if response.open_manual {
            let url =
                crate::ui_helpers::manual_url("troubleshooting.html", Some("network-data-dir"));
            crate::ui_helpers::open_url(&url);
        }
        if response.dismiss_for_path {
            self.settings.network_data_dir_notice_dismissed_for =
                Some(crate::data_dir::network_notice_path_value(&data_dir));
            self.settings.save();
        }
        if response.close || response.dismiss_for_path || !open || escape_pressed {
            self.network_data_dir_notice = None;
        }
    }
}
