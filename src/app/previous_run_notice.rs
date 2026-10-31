use crate::previous_run_diagnostics::{PreviousRunReport, PreviousRunReportHandle};

/// Process-wide notice; viewer open/switch/close does not consume it.
#[derive(Default)]
pub(crate) enum PreviousRunNotice {
    Waiting(PreviousRunReportHandle),
    Visible(PreviousRunReport),
    #[default]
    Closed,
}

impl PreviousRunNotice {
    fn poll(&mut self, drawable: bool) {
        if drawable
            && let Self::Waiting(handle) = self
            && let Some(report) = handle.take_report()
        {
            *self = Self::Visible(report);
        }
    }

    fn draw(&mut self, ctx: &egui::Context, drawable: bool) {
        self.poll(drawable);
        if drawable
            && let Self::Visible(body) = self
            && crate::ui_previous_run_notice::draw(ctx, body)
        {
            *self = Self::Closed;
        }
    }
}

impl super::App {
    pub(crate) fn install_previous_run_notice(
        &mut self,
        handle: Option<PreviousRunReportHandle>,
        ctx: &egui::Context,
    ) {
        self.previous_run_notice = match handle {
            Some(handle) => {
                handle.set_repaint_context(ctx);
                PreviousRunNotice::Waiting(handle)
            }
            None => PreviousRunNotice::Closed,
        };
    }

    pub(super) fn show_previous_run_notice(&mut self, ctx: &egui::Context) {
        let drawable = !self.indexer_init.overlay_active()
            && self.window_visible
            && !ctx.input(|input| input.viewport().minimized.unwrap_or(false))
            && !self.startup_notice_viewer_open()
            && !self.common_modal_dialog_open();
        self.previous_run_notice.draw(ctx, drawable);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_notice_keeps_the_worker_report_until_it_can_be_shown() {
        let handle = crate::previous_run_diagnostics::test_support::report_handle();
        let mut notice = PreviousRunNotice::Waiting(handle.clone());
        for _ in 0..3 {
            notice.poll(false);
            assert!(matches!(notice, PreviousRunNotice::Waiting(_)));
        }
        notice.poll(true);
        let PreviousRunNotice::Visible(body) = &notice else {
            panic!("missing report")
        };
        assert!(
            body.summary()
                .contains("前回の正常終了を確認できませんでした")
        );
        assert!(
            handle.take_report().is_none(),
            "one owner consumes the immutable report once"
        );
        let saved = body.clone();
        notice.poll(true);
        assert!(matches!(&notice, PreviousRunNotice::Visible(body) if body == &saved));
    }

    #[test]
    fn visible_report_survives_hidden_frames_and_is_immutable() {
        let body = PreviousRunReport::from_sections(
            "前回の正常終了を確認できませんでした。".into(),
            "詳細".into(),
        );
        let mut notice = PreviousRunNotice::Visible(body.clone());
        let ctx = egui::Context::default();
        for _ in 0..3 {
            notice.draw(&ctx, false);
            assert!(matches!(&notice, PreviousRunNotice::Visible(saved) if saved == &body));
        }
        notice = PreviousRunNotice::Closed;
        notice.poll(true);
        assert!(matches!(notice, PreviousRunNotice::Closed));
    }
}
