//! Arrivals remain attached to manual requests rather than the context or timer.
use super::handler_tests::{app, frame, seed};
use super::*;

#[test]
fn a_manual_boundary_does_not_attach_its_landing_to_a_due_slideshow() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx, SpreadMode::Single);
    app.settings.fullscreen_page_start_position =
        crate::settings::FullscreenPageStartPosition::Center;
    seed(&ctx, &mut app);
    app.slideshow_playing = true;
    app.slideshow_anchor_idx = Some(0);
    app.slideshow_next_at = std::time::Instant::now() - std::time::Duration::from_millis(1);
    frame(&ctx, vec![], |_, ctx| {
        app.handle_fs_navigation(
            ctx,
            false,
            false,
            None,
            None,
            None,
            FsPageNavRequest {
                nav: FsPageNav::Boundary { at_end: false },
                landing: Some(FsPageLanding::End),
            },
            None,
            0,
        );
    });
    assert_eq!(app.fullscreen_idx, Some(1));
    assert!(app.fs_initial_placement_request.is_none());
    assert_eq!(
        app.settings.fullscreen_page_start_position,
        crate::settings::FullscreenPageStartPosition::Center
    );
}
