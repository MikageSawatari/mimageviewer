//! App-global service progress. No UI input, presentation adoption, or UI mounts.
use super::App;

impl App {
    pub(super) fn service_app_logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Capture modal ownership before any receipt/terminal changes it.
        let lifecycle = self.accept_root_lifecycle_before_frame(ctx);
        self.diagnostics_export.poll();
        self.edit_preview_repaint_ctx = Some(ctx.clone());
        self.service_capture_main_window(ctx, frame);
        if self.settings.minimize_to_tray_on_close || self.tray_controller.is_some() {
            self.service_tray_requests(ctx);
        }
        self.service_root_lifecycle_tray_checkpoint(ctx, lifecycle);
        self.sync_tray_resident_media_wake();

        self.poll_composition_editor_save_receipts(ctx);
        self.resume_composition_editor_root_close(ctx);
        self.service_settings_family_operation(ctx);
        self.service_image_folder_zip_exit(ctx);
        self.service_discarded_sidecar_restore(ctx);
        self.poll_spread_shift_service();
        self.poll_book_resume_meters(ctx);
        self.poll_book_bookmark_service_responses();
        self.service_cancelled_ai_terminals();
        self.service_remote_session(ctx);
        #[cfg(windows)]
        self.service_detached_watcher_close_requests(ctx);
        self.service_folder_watch(ctx);
        self.service_periodic_folder_work(ctx);
        self.drain_gamepad_physical_input(ctx, std::time::Instant::now());
    }
}
