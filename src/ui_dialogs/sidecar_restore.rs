use crate::app::App;

pub(crate) fn draw_sidecar_restore_modal(
    ctx: &egui::Context,
    presentation: crate::app::SidecarRestorePresentation,
) {
    egui::Modal::new(egui::Id::new("sidecar_restore_modal")).show(ctx, |ui| {
        ui.set_min_width(340.0);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.heading(presentation.heading);
        });
        ui.add_space(6.0);
        ui.label(presentation.detail);
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
        let remaining = state.modal_delay_remaining(std::time::Instant::now());
        if !remaining.is_zero() {
            // Fast Missing/already-synchronized checks should not flash a one-frame dialog.
            // The restore state and its input gate are already active; only drawing waits.
            ctx.request_repaint_after(remaining);
            return;
        }
        draw_sidecar_restore_modal(ctx, state.presentation());
    }
}
