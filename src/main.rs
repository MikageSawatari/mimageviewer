#![windows_subsystem = "windows"]

fn main() -> eframe::Result {
    let result = mimageviewer::run();
    miv_startup::terminal(if result.is_ok() {
        miv_startup::Outcome::Ok
    } else {
        miv_startup::Outcome::Error
    });
    result
}
