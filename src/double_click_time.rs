#[cfg(any(windows, test))]
const EGUI_DEFAULT_DOUBLE_CLICK_DELAY_SECONDS: f64 = 0.3;

#[cfg(any(windows, test))]
fn windows_milliseconds_to_seconds(milliseconds: u32) -> f64 {
    if milliseconds == 0 {
        EGUI_DEFAULT_DOUBLE_CLICK_DELAY_SECONDS
    } else {
        f64::from(milliseconds) / 1_000.0
    }
}

#[cfg(any(windows, test))]
fn apply_to_context(ctx: &egui::Context, delay_seconds: f64) {
    ctx.options_mut(|options| {
        options.input_options.max_double_click_delay = delay_seconds;
    });
}

#[cfg(windows)]
fn startup_delay_seconds() -> f64 {
    use std::sync::OnceLock;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;

    static STARTUP_DELAY_SECONDS: OnceLock<f64> = OnceLock::new();
    *STARTUP_DELAY_SECONDS.get_or_init(|| {
        let milliseconds = unsafe { GetDoubleClickTime() };
        windows_milliseconds_to_seconds(milliseconds)
    })
}

/// Capture the Windows setting once during application startup.
///
/// Later contexts reuse the captured value, so changing the OS setting while
/// mImageViewer is running takes effect only after an application restart.
pub(crate) fn capture_startup_setting() {
    #[cfg(windows)]
    let _ = startup_delay_seconds();
}

/// Apply the startup double-click delay to a click-receiving egui context.
pub(crate) fn configure_context(ctx: &egui::Context) {
    #[cfg(windows)]
    apply_to_context(ctx, startup_delay_seconds());

    #[cfg(not(windows))]
    let _ = ctx;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_windows_milliseconds_to_seconds() {
        assert_eq!(windows_milliseconds_to_seconds(500), 0.5);
        assert_eq!(windows_milliseconds_to_seconds(0), 0.3);
        assert_eq!(windows_milliseconds_to_seconds(1_234), 1.234);
    }

    #[test]
    fn conversion_does_not_clamp_os_values() {
        assert_eq!(windows_milliseconds_to_seconds(1), 0.001);
        assert_eq!(
            windows_milliseconds_to_seconds(u32::MAX),
            f64::from(u32::MAX) / 1_000.0
        );
    }

    #[test]
    fn applies_delay_to_context_input_options() {
        let ctx = egui::Context::default();

        apply_to_context(&ctx, 0.875);

        assert_eq!(
            ctx.options(|options| options.input_options.max_double_click_delay),
            0.875
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_configures_context_from_captured_startup_setting() {
        capture_startup_setting();
        let ctx = egui::Context::default();

        configure_context(&ctx);

        assert_eq!(
            ctx.options(|options| options.input_options.max_double_click_delay),
            startup_delay_seconds()
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_keeps_egui_default() {
        let ctx = egui::Context::default();

        configure_context(&ctx);

        assert_eq!(
            ctx.options(|options| options.input_options.max_double_click_delay),
            EGUI_DEFAULT_DOUBLE_CLICK_DELAY_SECONDS
        );
    }
}
