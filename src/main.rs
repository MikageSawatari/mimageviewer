#![windows_subsystem = "windows"]

fn main() -> eframe::Result {
    let mut session = mimageviewer::GuiRunSession::default();
    let returned = mimageviewer::run(&mut session);
    mimageviewer::finish_gui_run(returned, &mut session)
}
