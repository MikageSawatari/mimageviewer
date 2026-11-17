use crate::app::App;

pub(crate) fn draw_sidecar_restore_modal(
    ctx: &egui::Context,
    presentation: crate::app::SidecarRestorePresentation,
) -> bool {
    egui::Modal::new(App::sidecar_restore_modal_layer().id)
        .show(ctx, |ui| {
            ui.set_min_width(340.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.heading(presentation.heading);
            });
            ui.add_space(6.0);
            ui.label(presentation.detail);
            ui.small("完了するまでこのままお待ちください。");
            // A new Area first measures its contents in an invisible sizing pass.
            ui.is_visible() && !ui.is_sizing_pass()
        })
        .inner
}

impl App {
    pub(crate) fn show_sidecar_restore_dialog(&self, ctx: &egui::Context) {
        self.show_sidecar_restore_dialog_at(ctx, std::time::Instant::now());
    }

    pub(crate) fn show_sidecar_restore_dialog_at(
        &self,
        ctx: &egui::Context,
        now: std::time::Instant,
    ) {
        if !self.sidecar_restore_blocks_projected_context() {
            return;
        }
        // Count only root viewport draws, where the main context is mounted.
        // Child viewports (direct or shared-helper draws) are outside this diagnostic metric;
        // modal_shown=false does not prove that a child viewport never displayed the Modal.
        let observe_root = ctx.viewport_id() == egui::ViewportId::ROOT;
        if let Some(started_at) = self.spread_shift.retained_readoption_started_at() {
            let remaining = crate::app::SIDECAR_RESTORE_MODAL_GRACE
                .saturating_sub(now.saturating_duration_since(started_at));
            if !remaining.is_zero() {
                ctx.request_repaint_after(remaining);
                return;
            }
            if draw_sidecar_restore_modal(
                ctx,
                crate::app::SidecarRestorePresentation {
                    heading: "表示を準備中",
                    detail: "保存された見開き調整を表示へ反映しています。",
                },
            ) && observe_root
            {
                self.spread_shift.offer_retained_readoption_modal_paint();
            }
            return;
        }
        let Some(state) = self.sidecar_restore.as_ref() else {
            return;
        };
        let remaining = state.modal_delay_remaining(now);
        if !remaining.is_zero() {
            // Fast Missing/already-synchronized checks should not flash a one-frame dialog.
            // The restore state and its input gate are already active; only drawing waits.
            ctx.request_repaint_after(remaining);
            return;
        }
        if draw_sidecar_restore_modal(ctx, state.presentation()) && observe_root {
            state.offer_modal_paint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_restore_draw_reports_only_visible_modal_passes() {
        let ctx = egui::Context::default();
        let presentation = crate::app::SidecarRestorePresentation {
            heading: "heading",
            detail: "detail",
        };
        let mut painted = false;
        let sizing = ctx.run_pass(egui::RawInput::default(), |ctx| {
            painted = draw_sidecar_restore_modal(ctx, presentation);
        });
        assert!(
            !painted,
            "the initial invisible sizing pass must not emit a shown event"
        );
        assert!(
            sizing
                .shapes
                .iter()
                .all(|shape| matches!(shape.shape, egui::Shape::Noop))
        );
        let visible = ctx.run_pass(egui::RawInput::default(), |ctx| {
            painted = draw_sidecar_restore_modal(ctx, presentation);
        });
        assert!(painted);
        assert!(
            visible
                .shapes
                .iter()
                .any(|shape| !matches!(shape.shape, egui::Shape::Noop))
        );
        sizing.drop_without_applying_deltas();
        visible.drop_without_applying_deltas();
    }
}
