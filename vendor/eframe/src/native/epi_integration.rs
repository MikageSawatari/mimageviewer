//! Common tools used by [`super::glow_integration`] and [`super::wgpu_integration`].

use web_time::Instant;

use std::path::PathBuf;
use winit::event_loop::ActiveEventLoop;

use raw_window_handle::{HasDisplayHandle as _, HasWindowHandle as _};

use egui::{DeferredViewportUiCallback, ViewportBuilder, ViewportId};
use egui_winit::{EventResponse, WindowSettings};

use crate::epi;

#[cfg_attr(target_os = "ios", allow(dead_code, unused_variables, unused_mut))]
pub fn viewport_builder(
    egui_zoom_factor: f32,
    event_loop: &ActiveEventLoop,
    native_options: &mut epi::NativeOptions,
    window_settings: Option<WindowSettings>,
) -> ViewportBuilder {
    profiling::function_scope!();

    let mut viewport_builder = native_options.viewport.clone();

    // On some Linux systems, a window size larger than the monitor causes crashes,
    // and on Windows the window does not appear at all.
    let clamp_size_to_monitor_size = viewport_builder.clamp_size_to_monitor_size.unwrap_or(true);

    // Always use the default window size / position on iOS. Trying to restore the previous position
    // causes the window to be shown too small.
    #[cfg(not(target_os = "ios"))]
    let inner_size_points = if let Some(mut window_settings) = window_settings {
        // Restore pos/size from previous session

        if clamp_size_to_monitor_size {
            window_settings.clamp_size_to_sane_values(largest_monitor_point_size(
                egui_zoom_factor,
                event_loop,
            ));
        }
        window_settings.clamp_position_to_monitors(egui_zoom_factor, event_loop);

        viewport_builder = window_settings.initialize_viewport_builder(
            egui_zoom_factor,
            event_loop,
            viewport_builder,
        );
        window_settings.inner_size_points()
    } else {
        if let Some(pos) = viewport_builder.position {
            viewport_builder = viewport_builder.with_position(pos);
        }

        if clamp_size_to_monitor_size && let Some(initial_window_size) = viewport_builder.inner_size
        {
            let initial_window_size = egui::NumExt::at_most(
                initial_window_size,
                largest_monitor_point_size(egui_zoom_factor, event_loop),
            );
            viewport_builder = viewport_builder.with_inner_size(initial_window_size);
        }

        viewport_builder.inner_size
    };

    #[cfg(not(target_os = "ios"))]
    if native_options.centered {
        profiling::scope!("center");
        if let Some(monitor) = event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next())
        {
            let monitor_size = monitor
                .size()
                .to_logical::<f32>(egui_zoom_factor as f64 * monitor.scale_factor());
            let inner_size = inner_size_points.unwrap_or(egui::Vec2 { x: 800.0, y: 600.0 });
            if 0.0 < monitor_size.width && 0.0 < monitor_size.height {
                let x = (monitor_size.width - inner_size.x) / 2.0;
                let y = (monitor_size.height - inner_size.y) / 2.0;
                viewport_builder = viewport_builder.with_position([x, y]);
            }
        }
    }

    match std::mem::take(&mut native_options.window_builder) {
        Some(hook) => hook(viewport_builder),
        None => viewport_builder,
    }
}

pub fn apply_window_settings(
    window: &winit::window::Window,
    window_settings: Option<WindowSettings>,
) {
    profiling::function_scope!();
    if let Some(window_settings) = window_settings {
        window_settings.initialize_window(window);
    }
}

#[cfg(not(target_os = "ios"))]
fn largest_monitor_point_size(egui_zoom_factor: f32, event_loop: &ActiveEventLoop) -> egui::Vec2 {
    profiling::function_scope!();
    let mut max_size = egui::Vec2::ZERO;

    let available_monitors = {
        profiling::scope!("available_monitors");
        event_loop.available_monitors()
    };

    for monitor in available_monitors {
        let size = monitor
            .size()
            .to_logical::<f32>(egui_zoom_factor as f64 * monitor.scale_factor());
        let size = egui::vec2(size.width, size.height);
        max_size = max_size.max(size);
    }

    if max_size == egui::Vec2::ZERO {
        egui::Vec2::splat(16000.0)
    } else {
        max_size
    }
}

// ----------------------------------------------------------------------------

/// For loading/saving app state and/or egui memory to disk.
pub fn create_storage(_app_name: &str) -> Option<Box<dyn epi::Storage>> {
    #[cfg(feature = "persistence")]
    if let Some(storage) = super::file_storage::FileStorage::from_app_id(_app_name) {
        return Some(Box::new(storage));
    }
    None
}

#[allow(clippy::allow_attributes, clippy::unnecessary_wraps)]
pub fn create_storage_with_file(_file: impl Into<PathBuf>) -> Option<Box<dyn epi::Storage>> {
    #[cfg(feature = "persistence")]
    return Some(Box::new(
        super::file_storage::FileStorage::from_ron_filepath(_file),
    ));
    #[cfg(not(feature = "persistence"))]
    None
}

// ----------------------------------------------------------------------------

/// Everything needed to make a winit-based integration for [`epi`].
///
/// Only one instance per app (not one per viewport).
pub struct EpiIntegration {
    pub frame: epi::Frame,
    last_auto_save: Instant,
    pub beginning: Instant,
    pub egui_ctx: egui::Context,
    pending_full_output: egui::FullOutput,

    /// When set, it is time to close the native window.
    close: bool,

    can_drag_window: bool,
    #[cfg(feature = "persistence")]
    persist_window: bool,
    app_icon_setter: super::app_icon::AppTitleIconSetter,
}

impl EpiIntegration {
    #[allow(clippy::allow_attributes, clippy::too_many_arguments)]
    pub fn new(
        egui_ctx: egui::Context,
        window: &winit::window::Window,
        app_name: &str,
        native_options: &crate::NativeOptions,
        storage: Option<Box<dyn epi::Storage>>,
        #[cfg(feature = "glow")] gl: Option<std::sync::Arc<glow::Context>>,
        #[cfg(feature = "glow")] glow_register_native_texture: Option<
            Box<dyn FnMut(glow::Texture) -> egui::TextureId>,
        >,
        #[cfg(feature = "wgpu")] wgpu_render_state: Option<egui_wgpu::RenderState>,
    ) -> Self {
        let frame = epi::Frame {
            info: epi::IntegrationInfo { cpu_usage: None },
            storage,
            #[cfg(feature = "glow")]
            gl,
            #[cfg(feature = "glow")]
            glow_register_native_texture,
            #[cfg(feature = "wgpu")]
            wgpu_render_state,
            raw_display_handle: window.display_handle().map(|h| h.as_raw()),
            raw_window_handle: window.window_handle().map(|h| h.as_raw()),
            native_window_visible_commit_completed: false,
        };

        let icon = native_options
            .viewport
            .icon
            .clone()
            .unwrap_or_else(|| std::sync::Arc::new(load_default_egui_icon()));

        let app_icon_setter = super::app_icon::AppTitleIconSetter::new(
            native_options
                .viewport
                .title
                .clone()
                .unwrap_or_else(|| app_name.to_owned()),
            Some(icon),
        );

        Self {
            frame,
            last_auto_save: Instant::now(),
            egui_ctx,
            pending_full_output: Default::default(),
            close: false,
            can_drag_window: false,
            #[cfg(feature = "persistence")]
            persist_window: native_options.persist_window,
            app_icon_setter,
            beginning: Instant::now(),
        }
    }

    /// If `true`, it is time to close the native window.
    pub fn should_close(&self) -> bool {
        self.close
    }

    pub fn on_window_event(
        &mut self,
        app: &mut dyn epi::App,
        viewport_id: ViewportId,
        window: &winit::window::Window,
        egui_winit: &mut egui_winit::State,
        event: &winit::event::WindowEvent,
    ) -> EventResponse {
        profiling::function_scope!(egui_winit::short_window_event_description(event));

        use winit::event::{ElementState, MouseButton, WindowEvent};

        if let WindowEvent::MouseInput {
            button: MouseButton::Left,
            state: ElementState::Pressed,
            ..
        } = event
        {
            self.can_drag_window = true;
        }

        let previous_event_count = egui_winit.egui_input().events.len();
        let response = egui_winit.on_window_event(window, event);
        notify_native_input_events(
            app,
            viewport_id,
            native_input_source(event),
            &egui_winit.egui_input().events,
            previous_event_count,
        );
        response
    }

    pub fn pre_update(&mut self) {
        self.app_icon_setter.update();
    }

    /// Run user code - this can create immediate viewports, so hold no locks over this!
    ///
    /// If `viewport_ui_cb` is None, we are in the root viewport and will call [`crate::App::update`].
    pub fn update(
        &mut self,
        app: &mut dyn epi::App,
        viewport_ui_cb: Option<&DeferredViewportUiCallback>,
        mut raw_input: egui::RawInput,
    ) -> egui::FullOutput {
        raw_input.time = Some(self.beginning.elapsed().as_secs_f64());

        let close_requested = raw_input.viewport().close_requested();

        app.raw_input_hook(&self.egui_ctx, &mut raw_input);

        let full_output = self.egui_ctx.run(raw_input, |egui_ctx| {
            if let Some(viewport_ui_cb) = viewport_ui_cb {
                // Child viewport
                profiling::scope!("viewport_callback");
                viewport_ui_cb(egui_ctx);
            } else {
                profiling::scope!("App::update");
                app.update(egui_ctx, &mut self.frame);
            }
        });

        let is_root_viewport = viewport_ui_cb.is_none();
        if is_root_viewport && close_requested {
            let canceled = full_output.viewport_output[&ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::CancelClose);
            if canceled {
                log::debug!("Closing of root viewport canceled with ViewportCommand::CancelClose");
            } else {
                log::debug!("Closing root viewport (ViewportCommand::CancelClose was not sent)");
                self.close = true;
            }
        }

        self.pending_full_output.append(full_output);
        std::mem::take(&mut self.pending_full_output)
    }

    pub fn report_frame_time(&mut self, seconds: f32) {
        self.frame.info.cpu_usage = Some(seconds);
    }

    pub fn post_rendering(&mut self, window: &winit::window::Window) {
        profiling::function_scope!();
        // The receipt belongs to ROOT even though this integration also paints children.
        let is_root = window.window_handle().ok().is_some_and(|handle| {
            self.frame
                .raw_window_handle
                .as_ref()
                .is_ok_and(|root| *root == handle.as_raw())
        });
        if is_root && !self.frame.native_window_visible_commit_completed() {
            #[cfg(target_os = "windows")]
            let diagnostic_hwnd = if crate::startup_window_observer::enabled() {
                window.window_handle().ok().and_then(|handle| {
                    if let raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw() {
                        Some(handle.hwnd.get() as usize)
                    } else {
                        None
                    }
                })
            } else {
                None
            };
            #[cfg(target_os = "windows")]
            if let Some(hwnd) = diagnostic_hwnd {
                crate::startup_window_observer::emit("eframe.first_paint.call_returned", hwnd);
                crate::startup_window_observer::emit("eframe.set_visible.begin", hwnd);
            }
            // We keep hidden until we've painted something. See https://github.com/emilk/egui/pull/2279
            self.frame
                .commit_initial_native_visibility(|| window.set_visible(true));
            #[cfg(target_os = "windows")]
            if let Some(hwnd) = diagnostic_hwnd {
                crate::startup_window_observer::emit("eframe.set_visible.complete", hwnd);
            }
        }
    }

    // ------------------------------------------------------------------------
    // Persistence stuff:

    pub fn maybe_autosave(
        &mut self,
        app: &mut dyn epi::App,
        window: Option<&winit::window::Window>,
    ) {
        let now = Instant::now();
        if now - self.last_auto_save > app.auto_save_interval() {
            self.save(app, window);
            self.last_auto_save = now;
        }
    }

    pub fn save(&mut self, app: &mut dyn epi::App, window: Option<&winit::window::Window>) {
        #[cfg(not(feature = "persistence"))]
        let _ = (self, app, window);

        #[cfg(feature = "persistence")]
        if let Some(storage) = self.frame.storage_mut() {
            profiling::function_scope!();

            if let Some(window) = window
                && self.persist_window
            {
                profiling::scope!("native_window");
                epi::set_value(
                    storage,
                    STORAGE_WINDOW_KEY,
                    &WindowSettings::from_window(self.egui_ctx.zoom_factor(), window),
                );
            }
            if app.persist_egui_memory() {
                profiling::scope!("egui_memory");
                self.egui_ctx
                    .memory(|mem| epi::set_value(storage, STORAGE_EGUI_MEMORY_KEY, mem));
            }
            {
                profiling::scope!("App::save");
                app.save(storage);
            }

            profiling::scope!("Storage::flush");
            storage.flush();
        }
    }
}

fn native_input_source(event: &winit::event::WindowEvent) -> epi::NativeInputSource {
    use winit::event::{ElementState, MouseButton, TouchPhase, WindowEvent};
    match event {
        WindowEvent::MouseInput {
            button: MouseButton::Left,
            state: ElementState::Released,
            ..
        } => epi::NativeInputSource::PrimaryMouseRelease,
        WindowEvent::Touch(touch) if touch.phase == TouchPhase::Ended => {
            epi::NativeInputSource::ContactEnd {
                id: touch.id,
                location: [touch.location.x.to_bits(), touch.location.y.to_bits()],
            }
        }
        _ => epi::NativeInputSource::Other,
    }
}

fn notify_native_input_events(
    app: &mut dyn epi::App,
    viewport_id: ViewportId,
    source: epi::NativeInputSource,
    events: &[egui::Event],
    previous_event_count: usize,
) {
    // Empty batches still retire native releases filtered by egui-winit. This
    // borrows the producer's exact suffix without copying, consuming, or editing it.
    app.native_input_events(viewport_id, source, &events[previous_event_count..]);
}

fn load_default_egui_icon() -> egui::IconData {
    profiling::function_scope!();
    crate::icon_data::from_png_bytes(&include_bytes!("../../data/icon.png")[..]).unwrap()
}

#[cfg(feature = "persistence")]
const STORAGE_EGUI_MEMORY_KEY: &str = "egui";

#[cfg(feature = "persistence")]
const STORAGE_WINDOW_KEY: &str = "window";

pub fn load_window_settings(_storage: Option<&dyn epi::Storage>) -> Option<WindowSettings> {
    profiling::function_scope!();
    #[cfg(feature = "persistence")]
    {
        epi::get_value(_storage?, STORAGE_WINDOW_KEY)
    }
    #[cfg(not(feature = "persistence"))]
    None
}

pub fn load_egui_memory(_storage: Option<&dyn epi::Storage>) -> Option<egui::Memory> {
    profiling::function_scope!();
    #[cfg(feature = "persistence")]
    {
        epi::get_value(_storage?, STORAGE_EGUI_MEMORY_KEY)
    }
    #[cfg(not(feature = "persistence"))]
    None
}

#[cfg(test)]
mod native_input_observer_tests {
    use super::*;
    use winit::event::{DeviceId, ElementState, MouseButton, Touch, TouchPhase, WindowEvent};

    #[test]
    fn native_input_source_maps_native_release_independently_of_raw_output() {
        let mouse = |button, state| WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            button,
            state,
        };
        assert_eq!(
            native_input_source(&mouse(MouseButton::Left, ElementState::Released)),
            epi::NativeInputSource::PrimaryMouseRelease
        );
        for event in [
            mouse(MouseButton::Left, ElementState::Pressed),
            mouse(MouseButton::Right, ElementState::Released),
            WindowEvent::CursorLeft {
                device_id: DeviceId::dummy(),
            },
        ] {
            assert_eq!(native_input_source(&event), epi::NativeInputSource::Other);
        }

        let touch = |phase| {
            WindowEvent::Touch(Touch {
                device_id: DeviceId::dummy(),
                phase,
                location: winit::dpi::PhysicalPosition::new(4.0, 5.0),
                force: None,
                id: u64::MAX,
            })
        };
        assert_eq!(
            native_input_source(&touch(TouchPhase::Ended)),
            epi::NativeInputSource::ContactEnd {
                id: u64::MAX,
                location: [4.0_f64.to_bits(), 5.0_f64.to_bits()],
            }
        );
        for phase in [
            TouchPhase::Started,
            TouchPhase::Moved,
            TouchPhase::Cancelled,
        ] {
            assert_eq!(
                native_input_source(&touch(phase)),
                epi::NativeInputSource::Other
            );
        }
    }

    #[test]
    fn native_input_source_contact_end_preserves_physical_location_bits() {
        let location = [-0.0_f64, 12.3456789012345_f64];
        let event = WindowEvent::Touch(Touch {
            device_id: DeviceId::dummy(),
            phase: TouchPhase::Ended,
            location: winit::dpi::PhysicalPosition::new(location[0], location[1]),
            force: None,
            id: 17,
        });
        assert_eq!(
            native_input_source(&event),
            epi::NativeInputSource::ContactEnd {
                id: 17,
                location: location.map(f64::to_bits),
            }
        );
    }

    #[derive(Default)]
    struct Observer {
        calls: usize,
        observed: Option<(
            ViewportId,
            epi::NativeInputSource,
            usize,
            *const egui::Event,
        )>,
    }

    impl epi::App for Observer {
        fn update(&mut self, _ctx: &egui::Context, _frame: &mut epi::Frame) {
            panic!("native input observation must not run the UI");
        }

        fn native_input_events(
            &mut self,
            viewport_id: ViewportId,
            source: epi::NativeInputSource,
            events: &[egui::Event],
        ) {
            self.calls += 1;
            self.observed = Some((viewport_id, source, events.len(), events.as_ptr()));
        }
    }

    #[test]
    fn native_input_observer_receives_exact_fresh_suffix_once_without_modification() {
        let mut observer = Observer::default();
        let viewport = ViewportId::from_hash_of("child observer");
        let events = vec![
            egui::Event::PointerGone,
            egui::Event::PointerMoved(egui::pos2(4.0, 5.0)),
            egui::Event::PointerButton {
                pos: egui::pos2(4.0, 5.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ];
        let original = events.clone();
        notify_native_input_events(
            &mut observer,
            viewport,
            epi::NativeInputSource::PrimaryMouseRelease,
            &events,
            1,
        );
        assert_eq!(observer.calls, 1);
        assert_eq!(
            observer.observed,
            Some((
                viewport,
                epi::NativeInputSource::PrimaryMouseRelease,
                2,
                events[1..].as_ptr()
            ))
        );
        assert_eq!(events, original);
    }

    #[test]
    fn native_input_observer_notifies_even_when_native_release_generated_no_raw_event() {
        let events = vec![egui::Event::PointerGone];
        for source in [
            epi::NativeInputSource::PrimaryMouseRelease,
            epi::NativeInputSource::ContactEnd {
                id: 17,
                location: [4.0_f64.to_bits(), 5.0_f64.to_bits()],
            },
            epi::NativeInputSource::Other,
        ] {
            let mut observer = Observer::default();
            notify_native_input_events(&mut observer, ViewportId::ROOT, source, &events, 1);
            assert_eq!(observer.calls, 1);
            assert_eq!(
                observer.observed,
                Some((ViewportId::ROOT, source, 0, events[1..].as_ptr()))
            );
        }
        assert_eq!(events, vec![egui::Event::PointerGone]);
    }

    #[test]
    fn native_input_observer_default_callback_leaves_input_unchanged() {
        struct DefaultObserver;
        impl epi::App for DefaultObserver {
            fn update(&mut self, _ctx: &egui::Context, _frame: &mut epi::Frame) {
                panic!("the default native input observer must not run the UI");
            }
        }
        let events = vec![egui::Event::PointerGone];
        notify_native_input_events(
            &mut DefaultObserver,
            ViewportId::ROOT,
            epi::NativeInputSource::PrimaryMouseRelease,
            &events,
            0,
        );
        assert_eq!(events, vec![egui::Event::PointerGone]);
    }
}
