//! Hidden real-HWND regression for mIV's borderless viewport contract (§1.426).
//! No window is shown or activated, and no application profile is accessed.

use egui::{ViewportBuilder, ViewportCommand, ViewportInfo};
use winit::{event_loop::EventLoop, platform::windows::EventLoopBuilderExtWindows as _};

fn client_matches_window(window: &winit::window::Window) -> (bool, bool) {
    // On Windows these query ClientToScreen/GetClientRect and GetWindowRect,
    // rather than the attributes requested at creation time. In winit 0.30.13,
    // MARKER_UNDECORATED_SHADOW shifts the client origin down by one pixel.
    (
        window.inner_position().expect("client screen origin")
            == window.outer_position().expect("window screen origin"),
        window.inner_size() == window.outer_size(),
    )
}

#[test]
fn undecorated_viewports_have_no_nonclient_shadow_at_creation_or_after_decorations_command() {
    let mut builder = EventLoop::builder();
    builder.with_any_thread(true);
    let event_loop = builder.build().expect("hidden HWND event loop");
    let ctx = egui::Context::default();
    let viewport = ViewportBuilder::default()
        .with_title("mIV hidden shadow regression")
        .with_visible(false)
        .with_active(false)
        .with_inner_size([320.0, 240.0])
        .with_decorations(false);
    // Exercise production attribute conversion without entering a visible app loop.
    #[expect(
        deprecated,
        reason = "hidden HWND test does not run an application loop"
    )]
    let window = event_loop
        .create_window(super::create_winit_window_attributes(&ctx, viewport))
        .expect("hidden borderless HWND");
    assert_eq!(window.is_visible(), Some(false));
    assert!(window.fullscreen().is_none());
    let at_creation = client_matches_window(&window);

    // Both commands use the production consumer. A decorated intermediate state
    // ensures this tests a genuine style transition, not a false -> false no-op.
    let mut info = ViewportInfo::default();
    let mut actions = Vec::new();
    super::process_viewport_commands(
        &ctx,
        &mut info,
        [ViewportCommand::Decorations(true)],
        &window,
        &mut actions,
    );
    assert!(window.is_decorated());
    assert_ne!(window.inner_size(), window.outer_size());
    super::process_viewport_commands(
        &ctx,
        &mut info,
        [ViewportCommand::Decorations(false)],
        &window,
        &mut actions,
    );
    assert!(!window.is_decorated());
    assert_eq!(window.is_visible(), Some(false));
    assert!(actions.is_empty());
    let after_command = client_matches_window(&window);
    assert_eq!(
        [at_creation, after_command],
        [(true, true); 2],
        "client origin and size must match the window at creation and after Decorations(true -> false)"
    );
}
