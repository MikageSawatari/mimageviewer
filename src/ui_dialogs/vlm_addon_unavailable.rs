//! Keep the portable CI compile path independent of the Windows-only worker.
//! DXGI/Job Object transport and its UI are available only in the Windows build.
use crate::app::App;
use eframe::egui;

#[derive(Default)]
pub(crate) struct VlmSetup;

impl VlmSetup {
    pub(crate) fn modal_open(&self) -> bool {
        false
    }
}

#[derive(Clone, Default)]
pub(crate) struct VlmPreferencesView;

impl App {
    pub(crate) fn request_create_ai_captions(&mut self, _ctx: &egui::Context, _background: bool) {}
    pub(crate) fn poll_vlm_setup(&mut self, _ctx: &egui::Context) {}
    pub(crate) fn show_vlm_addon_dialog(&mut self, _ctx: &egui::Context) {}
    pub(crate) fn refresh_vlm_preferences(&mut self, _ctx: &egui::Context) {}
    pub(crate) fn process_vlm_preferences_requests(&mut self, _ctx: &egui::Context) {}
}

pub(super) fn draw_preferences(
    ui: &mut egui::Ui,
    _view: &VlmPreferencesView,
) -> (bool, bool, bool) {
    ui.label("AI 説明文は Windows 版で利用できます。");
    (false, false, false)
}
