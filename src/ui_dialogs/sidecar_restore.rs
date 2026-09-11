use crate::app::App;

pub(crate) fn draw_sidecar_restore_modal(ctx: &egui::Context, detail: &str) {
    egui::Modal::new(egui::Id::new("sidecar_restore_modal")).show(ctx, |ui| {
        ui.set_min_width(340.0);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.heading("サイドカーから設定を復元中");
        });
        ui.add_space(6.0);
        ui.label(detail);
        ui.small("完了するまでこのままお待ちください。");
    });
}

impl App {
    pub(crate) fn show_sidecar_restore_dialog(&self, ctx: &egui::Context) {
        if !self.sidecar_restore_blocks_projected_context() {
            return;
        }
        let Some(state) = self.sidecar_restore.as_ref() else {
            return;
        };
        draw_sidecar_restore_modal(ctx, state.label());
    }
}
