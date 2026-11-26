use super::*;

struct CombineLabel;
impl Adapter for CombineLabel {
    fn label(&self) -> String {
        "image combine label".into()
    }
    fn open(&self, app: &mut App, ctx: &egui::Context) {
        app.move_grid_cursor(Some(0), GridCursorIntent::ExplicitKeep);
        app.open_image_combine_dialog(ctx);
        app.prepare_image_combine_typing_for_test();
    }
    fn locator(&self) -> Locator {
        Locator::Id(egui::Id::unique("image_combine_label"))
    }
    fn draft<'a>(&self, app: &'a App) -> &'a str {
        app.image_combine_typing_state_for_test().0
    }
    fn opens_focused(&self) -> bool {
        false
    }
    fn opening_frame(&self) -> u64 {
        DIALOG_OPENING_FRAME
    }
    fn job(&self) -> Option<&'static str> {
        Some("image_combine_label")
    }
    fn outcome(&self, _query: &str) -> Outcome {
        Outcome::Delivered(1)
    }
    fn adopted(&self, app: &App, query: &str, _outcome: Outcome) -> bool {
        let (label, rendered, count) = app.image_combine_typing_state_for_test();
        label == query && rendered && count == 1
    }
}

fn harness() -> Harness<CombineLabel> {
    let mut env = fixture();
    let paths = super::search::seed_images(&mut env);
    Harness::new_with_measurement(env, CombineLabel, paths, false)
}

#[test]
fn typing_image_combine_label_preview_ascii() {
    harness().type_text("alp");
}

#[test]
fn typing_image_combine_label_preview_ime() {
    harness().compose_ime(&["k", "か", "かな"], "かな");
}
