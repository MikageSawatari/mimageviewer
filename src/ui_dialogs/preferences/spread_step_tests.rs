use super::*;
use crate::{
    app::spread_shift_owner::SpreadStep,
    grid_item::GridItem,
    settings::{PageAlonePreference, SpreadMode},
};

fn seed(app: &mut crate::app::App) {
    app.current_folder = Some(std::path::PathBuf::from("c:/step-preferences"));
    app.items = (0..6)
        .map(|i| {
            GridItem::Image(std::path::PathBuf::from(format!(
                "c:/step-preferences/{i}.png"
            )))
        })
        .collect();
    app.visible_indices = (0..6).collect();
    app.thumbnails = vec![crate::grid_item::ThumbnailState::Pending; 6];
    app.image_metas = vec![None; 6];
    app.spread_mode = SpreadMode::LtrCover;
    app.fullscreen_idx = Some(1);
    app.spread_step = Some(SpreadStep { start_idx: 1 });
    app.settings.page_after_cover_alone_enabled = false;
}

#[test]
fn spread_step_global_page_alone_apply_retires_mounted_and_inheriting_parked_only() {
    for apply in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        seed(&mut app);
        #[cfg(windows)]
        let inherited = app.build_window_context_for_test(1801, seed);
        #[cfg(windows)]
        let explicit = app.build_window_context_for_test(1802, |owner| {
            seed(owner);
            owner.page_alone_preferences.after_cover = PageAlonePreference::On;
        });
        app.open_preferences_page(PreferencesPage::General);
        app.ensure_preferences_state();
        app.pref_state
            .as_mut()
            .unwrap()
            .settings
            .page_after_cover_alone_enabled = true;
        if apply {
            app.apply_preferences_dialog(&egui::Context::default());
        } else {
            app.discard_preferences_dialog();
        }
        assert_eq!(app.spread_step.is_none(), apply);
        #[cfg(windows)]
        {
            app.with_window_context_for_test(inherited, |owner| {
                assert_eq!(owner.spread_step.is_none(), apply)
            });
            app.with_window_context_for_test(explicit, |owner| {
                assert_eq!(owner.spread_step.unwrap().start_idx, 1)
            });
        }
    }
}
