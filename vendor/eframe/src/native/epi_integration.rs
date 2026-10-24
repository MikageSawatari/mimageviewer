//! Common tools used by [`super::glow_integration`] and [`super::wgpu_integration`].

use web_time::Instant;

use std::{path::PathBuf, sync::Arc};
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
    window_settings: &mut Option<WindowSettings>,
) -> ViewportBuilder {
    profiling::function_scope!();

    let mut viewport_builder = native_options.viewport.clone();

    // On some Linux systems, a window size larger than the monitor causes crashes,
    // and on Windows the window does not appear at all.
    let clamp_size_to_monitor_size = viewport_builder.clamp_size_to_monitor_size.unwrap_or(true);

    // Always use the default window size / position on iOS. Trying to restore the previous position
    // causes the window to be shown too small.
    #[cfg(not(target_os = "ios"))]
    let inner_size_points = if let Some(window_settings) = window_settings {
        // Restore pos/size from previous session

        if clamp_size_to_monitor_size {
            window_settings.clamp_size_to_sane_values(largest_monitor_point_size(
                egui_zoom_factor,
                event_loop,
            ));
        }
        window_settings.clamp_position_to_monitors(egui_zoom_factor, event_loop);

        viewport_builder =
            window_settings.initialize_viewport_builder(egui_zoom_factor, viewport_builder);
        Some(window_settings.inner_size_points())
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
                let x = (monitor_size.width - inner_size.x) * 0.5;
                let y = (monitor_size.height - inner_size.y) * 0.5;
                viewport_builder = viewport_builder.with_position([x, y]);
            }
        }
    }

    match core::mem::take(&mut native_options.window_builder) {
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

/// Stands in for the system font provider when the `system_fonts` feature is off.
///
/// It never finds a font, but tells you once why nothing was even looked for,
/// so missing CJK/Arabic/emoji glyphs don't look like an egui bug.
#[cfg(not(feature = "system_fonts"))]
#[derive(Default)]
struct MissingSystemFontsWarning {
    warned: core::sync::atomic::AtomicBool,
}

#[cfg(not(feature = "system_fonts"))]
impl egui::FontProvider for MissingSystemFontsWarning {
    fn font_for(&self, request: &egui::FallbackRequest<'_>) -> Option<egui::FontInsert> {
        use core::sync::atomic::Ordering::Relaxed;
        if !self.warned.swap(true, Relaxed) {
            log::info!(
                "No font has {:?}. \
                 Enable the `system_fonts` feature in `eframe`, \
                 add a font with `egui::Context::add_font`, \
                 or set `NativeOptions::system_font_fallback` to `false` to silence this.",
                request.cluster
            );
        }
        None
    }
}

// ----------------------------------------------------------------------------

/// Take only this viewport's unconsumed input, then append its newest State input.
/// The caller must release any viewport borrow/lock before running user code.
pub(super) fn take_viewport_input(
    pending: &mut egui::RawInput,
    take_new: impl FnOnce() -> egui::RawInput,
) -> egui::RawInput {
    let mut input = core::mem::take(pending);
    input.append(take_new());
    input
}

/// A skipped child keeps its input in State: even taking it would consume it.
/// Root logic-only still takes input so it can return the unconsumed batch.
pub(super) fn take_viewport_input_if(
    should_take: bool,
    pending: &mut egui::RawInput,
    take_new: impl FnOnce() -> egui::RawInput,
) -> Option<egui::RawInput> {
    should_take.then(|| take_viewport_input(pending, take_new))
}

/// Replacing a native State does not end its logical viewport's input lifetime.
/// A skipped child may still have terminal events queued in that State.
pub(super) fn retain_viewport_input_before_state_replacement(
    viewport_id: ViewportId,
    pending: &mut egui::RawInput,
    state: &mut egui_winit::State,
) {
    *pending = take_viewport_input(pending, || {
        let mut input = state.egui_input_mut().take();
        // State::new leaves RawInput's default ROOT id until the first window
        // take. Use the same owner id as State::take_egui_input, including when
        // rebuilding before the child's first Full pass. Normal acquisition
        // will refresh the complete live-viewport metadata before the hook.
        input.viewport_id = viewport_id;
        input.viewports.entry(viewport_id).or_default();
        input
    });
}

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

    /// Settings the root window was restored with, for the parts applied when it is first shown.
    restored_window_settings: Option<WindowSettings>,

    /// Normal (non-maximized) geometry of the root window, as last saved.
    #[cfg(feature = "persistence")]
    normal_geometry: Option<egui_winit::WindowGeometry>,

    app_icon_setter: super::app_icon::AppTitleIconSetter,
}

/// Feature-limited noninteractive driver for application integration regressions.
/// Runs the real EpiIntegration logic/UI and root Close decision. The caller owns
/// output delivery; this driver creates no windows, renderer, profile or files.
#[cfg(feature = "miv-test-script-window-witness")]
#[doc(hidden)]
pub struct HeadlessEpiIntegration(EpiIntegration);

#[cfg(feature = "miv-test-script-window-witness")]
impl HeadlessEpiIntegration {
    /// Create a windowless driver using the supplied Context.
    pub fn new(ctx: egui::Context) -> Self {
        Self(EpiIntegration::headless(ctx))
    }
    /// The same Context used by real logic and UI processing.
    pub fn context(&self) -> &egui::Context {
        &self.0.egui_ctx
    }
    /// Run one real root Full operation; the caller owns returned output.
    pub fn update(&mut self, app: &mut dyn epi::App, input: egui::RawInput) -> egui::FullOutput {
        self.0.update(app, None, input)
    }
    /// Run the native wgpu ROOT Full boundary, including its post-retry hook and
    /// texture-delta handoff. Does not create a renderer or submit GPU work.
    #[cfg(feature = "wgpu_no_default_features")]
    pub fn update_native_wgpu(
        &mut self,
        app: &mut dyn epi::App,
        input: egui::RawInput,
    ) -> egui::FullOutput {
        assert_eq!(input.viewport_id, egui::ViewportId::ROOT);
        let producing_root_frame = self
            .0
            .egui_ctx
            .cumulative_frame_nr_for(egui::ViewportId::ROOT);
        let mut output = self.0.update(app, None, input);
        super::wgpu_integration::finalize_native_wgpu_root_output(
            app,
            &self.0.egui_ctx,
            egui::ViewportId::ROOT,
            producing_root_frame,
            &mut output,
        );
        output
    }
    /// Run real root logic without UI, preserving the unconsumed input.
    pub fn update_logic_only(
        &mut self,
        app: &mut dyn epi::App,
        input: egui::RawInput,
    ) -> (egui::LogicOutput, egui::RawInput) {
        self.0.update_logic_only(app, input)
    }
    /// The backend root-close decision after the last completed operation.
    pub fn should_close(&self) -> bool {
        self.0.should_close()
    }
}

impl EpiIntegration {
    #[cfg(any(test, feature = "miv-test-script-window-witness"))]
    fn headless(egui_ctx: egui::Context) -> Self {
        egui_ctx.set_embed_viewports(false);
        egui_ime::install_ime_input_policy(&egui_ctx);
        Self {
            frame: epi::Frame {
                info: epi::IntegrationInfo { cpu_usage: None },
                storage: None,
                #[cfg(feature = "glow")]
                gl: None,
                #[cfg(feature = "glow")]
                glow_register_native_texture: None,
                #[cfg(feature = "wgpu_no_default_features")]
                wgpu_render_state: None,
                #[cfg(feature = "wgpu_no_default_features")]
                wgpu_surface_config: None,
                window: None,
                raw_display_handle: Err(raw_window_handle::HandleError::Unavailable),
                raw_window_handle: Err(raw_window_handle::HandleError::Unavailable),
                native_window_visible_commit_completed: false,
                activation_token: None,
            },
            last_auto_save: Instant::now(),
            beginning: Instant::now(),
            egui_ctx,
            pending_full_output: Default::default(),
            close: false,
            can_drag_window: false,
            #[cfg(feature = "persistence")]
            persist_window: false,
            restored_window_settings: None,
            #[cfg(feature = "persistence")]
            normal_geometry: None,
            app_icon_setter: super::app_icon::AppTitleIconSetter::new(String::new(), None),
        }
    }

    #[allow(clippy::allow_attributes, clippy::too_many_arguments)]
    pub fn new(
        egui_ctx: egui::Context,
        window: &Arc<winit::window::Window>,
        app_name: &str,
        native_options: &crate::NativeOptions,
        storage: Option<Box<dyn epi::Storage>>,
        #[cfg(feature = "glow")] gl: Option<std::sync::Arc<glow::Context>>,
        #[cfg(feature = "glow")] glow_register_native_texture: Option<
            Box<dyn FnMut(glow::Texture) -> egui::TextureId>,
        >,
        #[cfg(feature = "wgpu_no_default_features")] wgpu_render_state: Option<
            egui_wgpu::RenderState,
        >,
    ) -> Self {
        let frame = epi::Frame {
            info: epi::IntegrationInfo { cpu_usage: None },
            storage,
            #[cfg(feature = "glow")]
            gl,
            #[cfg(feature = "glow")]
            glow_register_native_texture,
            #[cfg(feature = "wgpu_no_default_features")]
            // We only care about the surface config if we are using wgpu
            wgpu_surface_config: wgpu_render_state
                .is_some()
                .then_some(native_options.wgpu_options.surface),
            #[cfg(feature = "wgpu_no_default_features")]
            wgpu_render_state,
            window: Some(Arc::clone(window)),
            raw_display_handle: window.display_handle().map(|h| h.as_raw()),
            raw_window_handle: window.window_handle().map(|h| h.as_raw()),
            native_window_visible_commit_completed: false,
            activation_token: None,
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

        if native_options.system_font_fallback {
            egui_ctx.add_font_provider(cfg_select! {
                feature = "system_fonts" => Arc::new(egui_system_font_provider::SystemFontProvider::new()),
                _ => Arc::new(MissingSystemFontsWarning::default()),
            });
        }

        let restored_window_settings = load_window_settings(frame.storage());
        #[cfg(feature = "persistence")]
        let normal_geometry = restored_window_settings
            .map(|settings| settings.normal_geometry())
            .or_else(|| {
                // Nothing saved yet, so use the window's own geometry, unless it starts maximized:
                (!window.is_maximized() && window.fullscreen().is_none()).then(|| {
                    egui_winit::WindowGeometry::from_window(egui_ctx.zoom_factor(), window)
                })
            });

        Self {
            frame,
            last_auto_save: Instant::now(),
            pending_full_output: Default::default(),
            close: false,
            can_drag_window: false,
            #[cfg(feature = "persistence")]
            persist_window: native_options.persist_window,
            restored_window_settings,
            #[cfg(feature = "persistence")]
            normal_geometry,
            app_icon_setter,
            beginning: Instant::now()
                .checked_sub(web_time::Duration::from_secs_f64(egui_ctx.time()))
                .unwrap_or_else(Instant::now),
            egui_ctx,
        }
    }

    /// If `true`, it is time to close the native window.
    pub fn should_close(&self) -> bool {
        self.close
    }

    pub fn on_window_event(
        &mut self,
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

        egui_winit.on_window_event(window, event)
    }

    pub fn pre_update(&mut self) {
        self.app_icon_setter.update();
    }

    /// Run user code - this can create immediate viewports, so hold no locks over this!
    ///
    /// If `viewport_ui_cb` is None, we are in the root viewport and will call
    /// [`crate::App::logic`] and [`crate::App::ui`].
    ///
    /// Only call this when the ui will actually be shown;
    /// use [`Self::update_logic_only`] otherwise.
    pub fn update(
        &mut self,
        app: &mut dyn epi::App,
        viewport_ui_cb: Option<&DeferredViewportUiCallback>,
        raw_input: egui::RawInput,
    ) -> egui::FullOutput {
        let raw_input = self.prepare_raw_input(app, raw_input);

        let close_requested = raw_input.viewport().close_requested();

        let is_root_viewport = viewport_ui_cb.is_none();

        let full_output = self.egui_ctx.run_ui(raw_input, |ui| {
            if let Some(viewport_ui_cb) = viewport_ui_cb {
                // Child viewport
                profiling::scope!("viewport_callback");
                viewport_ui_cb(ui);
            } else {
                {
                    profiling::scope!("App::logic");
                    app.logic(ui.ctx(), &mut self.frame);
                }
                {
                    profiling::scope!("App::ui");
                    app.ui(ui, &mut self.frame);
                }
            }
        });

        if is_root_viewport && close_requested {
            let canceled = full_output.viewport_output[&ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::CancelClose);
            self.handle_close_request(canceled);
        }

        self.pending_full_output.append(full_output);
        core::mem::take(&mut self.pending_full_output)
    }

    /// Let the app tick its logic without showing any ui,
    /// because the window is minimized or occluded.
    ///
    /// No egui pass is run, so all ui state is left untouched:
    /// the app will find everything where it left it once the window is visible again.
    ///
    /// Only call this for the root viewport: only it has [`crate::App::logic`].
    /// Returns the post-hook input, which no UI pass consumed. The caller must
    /// return it to the same root viewport's pending input before its next tick.
    pub fn update_logic_only(
        &mut self,
        app: &mut dyn epi::App,
        raw_input: egui::RawInput,
    ) -> (egui::LogicOutput, egui::RawInput) {
        let raw_input = self.prepare_raw_input(app, raw_input);

        let close_requested = raw_input.viewport().close_requested();

        let logic_output = self.egui_ctx.run_logic(&raw_input, |ctx| {
            profiling::scope!("App::logic");
            app.logic(ctx, &mut self.frame);
        });

        if close_requested {
            let canceled = logic_output
                .viewport_commands
                .get(&ViewportId::ROOT)
                .is_some_and(|commands| commands.contains(&egui::ViewportCommand::CancelClose));
            self.handle_close_request(canceled);
        }

        (logic_output, raw_input)
    }

    /// Set the time and run the app hook on the viewport's already-combined input.
    fn prepare_raw_input(
        &mut self,
        app: &mut dyn epi::App,
        mut raw_input: egui::RawInput,
    ) -> egui::RawInput {
        raw_input.time = Some(self.beginning.elapsed().as_secs_f64());

        app.raw_input_hook(&self.egui_ctx, &mut raw_input);

        raw_input
    }

    fn handle_close_request(&mut self, canceled: bool) {
        if canceled {
            log::debug!("Closing of root viewport canceled with ViewportCommand::CancelClose");
        } else {
            log::debug!("Closing root viewport (ViewportCommand::CancelClose was not sent)");
            self.close = true;
        }
    }

    pub fn report_frame_time(&mut self, seconds: f32) {
        self.frame.info.cpu_usage = Some(seconds);
    }

    pub fn post_rendering(&mut self, window: &winit::window::Window) {
        profiling::function_scope!();
        // Frame's native handle belongs to ROOT. A child paint cannot publish its receipt.
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
            }
            if let Some(window_settings) = &self.restored_window_settings {
                window_settings.before_first_show(window);
            }
            #[cfg(target_os = "windows")]
            if let Some(hwnd) = diagnostic_hwnd {
                crate::startup_window_observer::emit("eframe.set_visible.begin", hwnd);
            }
            self.frame
                .commit_initial_native_visibility(|| window.set_visible(true));
            #[cfg(target_os = "windows")]
            if let Some(hwnd) = diagnostic_hwnd {
                crate::startup_window_observer::emit("eframe.set_visible.complete", hwnd);
            }
            if let Some(window_settings) = &self.restored_window_settings {
                window_settings.after_first_show(window);
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
                let window_settings = WindowSettings::from_window(
                    self.egui_ctx.zoom_factor(),
                    window,
                    self.normal_geometry,
                );
                self.normal_geometry = Some(window_settings.normal_geometry());
                epi::set_value(storage, STORAGE_WINDOW_KEY, &window_settings);
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

fn load_default_egui_icon() -> egui::IconData {
    profiling::function_scope!();
    #[expect(clippy::unwrap_used)]
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

/// Return unconsumed root input to its owner, then consume this tick's native
/// lifecycle events. The next acquisition supplies the current metadata again.
pub(super) fn finish_root_logic_input(
    pending: &mut egui::RawInput,
    info: &mut egui::ViewportInfo,
    unused: egui::RawInput,
) {
    *pending = unused;
    info.events.clear();
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use egui::{Event, Key, Modifiers, RawInput};
    use std::sync::Mutex;

    fn child_id() -> ViewportId {
        ViewportId::from_hash_of("input-owner-child")
    }

    fn input(id: ViewportId, events: Vec<Event>) -> RawInput {
        let mut input = RawInput {
            viewport_id: id,
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            )),
            ..Default::default()
        };
        input.viewports.insert(child_id(), Default::default());
        input
    }

    fn text(value: &str) -> Event {
        Event::Text(value.to_owned())
    }

    fn key(key: Key, pressed: bool) -> Event {
        Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    /// Same ownership boundary as the native Viewport, with a fake State drain.
    /// Skips exercise the production selector, rather than a copied condition.
    struct InputOwner {
        pending: RawInput,
        state: RawInput,
        takes: usize,
    }

    impl InputOwner {
        fn new(id: ViewportId) -> Self {
            Self {
                pending: Default::default(),
                state: input(id, vec![]),
                takes: 0,
            }
        }

        fn take_if(&mut self, should_take: bool) -> Option<RawInput> {
            take_viewport_input_if(should_take, &mut self.pending, || {
                self.takes += 1;
                self.state.take()
            })
        }

        fn take(&mut self) -> RawInput {
            // Immediate rendering uses this same unconditional acquisition entry.
            take_viewport_input(&mut self.pending, || {
                self.takes += 1;
                self.state.take()
            })
        }
    }

    pub(in crate::native) fn headless_integration() -> EpiIntegration {
        EpiIntegration::headless(egui::Context::default())
    }

    #[derive(Default)]
    struct RecordingApp {
        hooks: Vec<(ViewportId, Vec<Event>)>,
        root_passes: Vec<Vec<Event>>,
        logic_calls: usize,
    }

    impl epi::App for RecordingApp {
        fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut RawInput) {
            self.hooks
                .push((raw_input.viewport_id, raw_input.events.clone()));
            // A hook mutation from LogicOnly must survive in the owner's pending batch.
            for event in &mut raw_input.events {
                if *event == text("A") {
                    *event = text("a");
                }
            }
        }

        fn logic(&mut self, _ctx: &egui::Context, _frame: &mut epi::Frame) {
            self.logic_calls += 1;
        }

        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut epi::Frame) {
            self.root_passes
                .push(ui.input(|input| input.events.clone()));
            ui.ctx()
                .show_viewport_deferred(child_id(), ViewportBuilder::default(), |_, _| {});
        }
    }

    #[test]
    fn viewport_input_logic_only_child_full_root_full_preserves_owner_and_hook_order() {
        for logic_ticks in [1, 3] {
            let mut integration = headless_integration();
            let mut app = RecordingApp::default();
            let mut root = InputOwner::new(ViewportId::ROOT);
            let mut child = InputOwner::new(child_id());
            let child_passes = Arc::new(Mutex::new(Vec::new()));
            let recorded = Arc::clone(&child_passes);
            let callback = move |ui: &mut egui::Ui| {
                recorded
                    .lock()
                    .unwrap()
                    .push(ui.input(|i| i.events.clone()));
            };
            let mut expected_root = Vec::new();
            let mut expected_hooks = Vec::new();

            for tick in 0..logic_ticks {
                root.state.events.push(text("A"));
                let mut before_hook = expected_root.clone();
                before_hook.push(text("A"));
                expected_hooks.push((ViewportId::ROOT, before_hook));
                let (_, unused) =
                    integration.update_logic_only(&mut app, root.take_if(true).unwrap());
                root.pending = unused;
                expected_root.push(text("a"));
                assert!(app.root_passes.is_empty(), "logic must not invoke UI");

                let event = text(&format!("B{tick}"));
                child.state.events.push(event.clone());
                expected_hooks.push((child_id(), vec![event.clone()]));
                integration
                    .update(&mut app, Some(&callback), child.take_if(true).unwrap())
                    .drop_without_applying_deltas();
                assert_eq!(child_passes.lock().unwrap()[tick], vec![event]);
                assert_eq!(root.pending.events, expected_root);
            }

            root.state.events.push(text("C"));
            expected_root.push(text("C"));
            expected_hooks.push((ViewportId::ROOT, expected_root.clone()));
            integration
                .update(&mut app, None, root.take_if(true).unwrap())
                .drop_without_applying_deltas();
            assert_eq!(app.root_passes, vec![expected_root]);
            assert_eq!(app.hooks, expected_hooks);
            assert_eq!(app.logic_calls, logic_ticks + 1);

            root.state.events.push(text("D"));
            integration
                .update(&mut app, None, root.take())
                .drop_without_applying_deltas();
            assert_eq!(app.root_passes.last().unwrap(), &vec![text("D")]);
            assert!(root.pending.events.is_empty());
            assert!(child.pending.events.is_empty());
        }
    }

    #[test]
    fn viewport_input_skips_do_not_drain_and_resume_appends_new_input_after_terminal_events() {
        let mut child = InputOwner::new(child_id());
        child.pending = input(child_id(), vec![text("pending")]);
        let old_rect = child.state.screen_rect;
        child.state.events = vec![key(Key::A, false), Event::WindowFocused(false)];
        assert!(child.take_if(false).is_none());
        child
            .state
            .events
            .push(Event::ModifiersChanged(Modifiers::NONE));
        assert!(child.take_if(false).is_none());
        assert_eq!(child.takes, 0);
        assert_eq!(child.pending.events, vec![text("pending")]);

        let mut sibling = InputOwner::new(ViewportId::ROOT);
        sibling.pending = input(ViewportId::ROOT, vec![text("sibling-pending")]);
        sibling.state.events.push(text("sibling-new"));
        let sibling_input = sibling.take();
        assert_eq!(
            sibling_input.events,
            vec![text("sibling-pending"), text("sibling-new")]
        );
        assert_eq!(child.takes, 0);

        child
            .state
            .events
            .extend([Event::WindowFocused(true), text("new")]);
        child.state.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(640.0, 480.0),
        ));
        assert_ne!(child.state.screen_rect, old_rect);
        let expected_rect = child.state.screen_rect;
        let resumed = child.take_if(true).unwrap();
        assert_eq!(resumed.viewport_id, child_id());
        assert_eq!(resumed.screen_rect, expected_rect);
        assert_eq!(
            resumed.events,
            vec![
                text("pending"),
                key(Key::A, false),
                Event::WindowFocused(false),
                Event::ModifiersChanged(Modifiers::NONE),
                Event::WindowFocused(true),
                text("new"),
            ]
        );
        assert_eq!(child.takes, 1);
        assert!(
            child.take().events.is_empty(),
            "resumed input is consumed exactly once"
        );

        sibling.pending = input(ViewportId::ROOT, vec![text("survives-child-close")]);
        drop(child);
        assert_eq!(sibling.take().events, vec![text("survives-child-close")]);
    }

    #[derive(Default)]
    struct TextField {
        text: String,
        request_initial_focus: bool,
        focused: bool,
        composing: bool,
        keys_down: std::collections::HashSet<Key>,
        modifiers: Modifiers,
        events: Vec<Event>,
    }

    #[test]
    #[allow(deprecated, reason = "Disabled is the Windows cancellation terminal")]
    fn viewport_input_state_replacement_retains_queued_terminal_events_and_owner() {
        for state_has_taken_input in [false, true] {
            let integration = headless_integration();
            let mut state = egui_winit::State::new(
                integration.egui_ctx.clone(),
                child_id(),
                &integration.frame,
                Some(1.5),
                None,
                None,
            );
            // A newly created child State really starts with ROOT in its raw
            // buffer. This must not relabel the surviving viewport's pending
            // events if it is rebuilt before its first Full input take.
            assert_eq!(state.egui_input().viewport_id, ViewportId::ROOT);
            if state_has_taken_input {
                state.egui_input_mut().viewport_id = child_id();
            }

            let mut pending = input(child_id(), vec![text("older-pending")]);
            pending.focused = true;
            pending.max_texture_side = Some(1024);
            let terminal_events = vec![
                key(Key::A, false),
                Event::WindowFocused(false),
                Event::ModifiersChanged(Modifiers::NONE),
                Event::Ime(egui::ImeEvent::Disabled),
            ];
            let latest_rect = Some(egui::Rect::from_min_size(
                egui::pos2(10.0, 20.0),
                egui::vec2(640.0, 480.0),
            ));
            let queued = state.egui_input_mut();
            queued.events = terminal_events.clone();
            queued.focused = false;
            queued.max_texture_side = Some(4096);
            queued.system_theme = Some(egui::Theme::Dark);
            queued.screen_rect = latest_rect;
            queued.time = Some(42.0);
            queued.predicted_dt = 0.02;

            retain_viewport_input_before_state_replacement(child_id(), &mut pending, &mut state);

            let mut expected_events = vec![text("older-pending")];
            expected_events.extend(terminal_events);
            assert_eq!(pending.viewport_id, child_id());
            assert!(pending.viewports.contains_key(&child_id()));
            assert_eq!(pending.events, expected_events);
            assert!(!pending.focused);
            assert_eq!(pending.max_texture_side, Some(4096));
            assert_eq!(pending.system_theme, Some(egui::Theme::Dark));
            assert_eq!(pending.screen_rect, latest_rect);
            assert_eq!(pending.time, Some(42.0));
            assert_eq!(pending.predicted_dt, 0.02);
            assert!(state.egui_input().events.is_empty());
            assert_eq!(state.egui_input().screen_rect, None);

            // Retiring the native State leaves the logical owner's single
            // pending batch intact, then the next State contributes after it.
            drop(state);
            expected_events.push(text("new-state-input"));
            let next_input = take_viewport_input(&mut pending, || {
                input(child_id(), vec![text("new-state-input")])
            });
            assert_eq!(next_input.viewport_id, child_id());
            assert_eq!(next_input.events, expected_events);
            assert!(pending.events.is_empty());
        }
    }

    impl TextField {
        fn show(&mut self, ui: &mut egui::Ui) {
            self.events = ui.input(|i| i.events.clone());
            self.keys_down = ui.input(|i| i.keys_down.clone());
            self.modifiers = ui.input(|i| i.modifiers);
            self.composing = egui_ime::ime_input_active(ui.ctx());
            let response = ui.add(egui::TextEdit::singleline(&mut self.text));
            if self.request_initial_focus {
                response.request_focus();
                self.request_initial_focus = false;
            }
            self.focused = response.has_focus();
        }
    }

    #[test]
    #[allow(deprecated, reason = "Disabled is the Windows cancellation terminal")]
    fn viewport_input_child_skip_preserves_keys_focus_ime_terminal_and_real_text_edit() {
        // Both commit and cancellation must terminate the composition while new
        // text after restoration remains editable in the same pass.
        for terminal in [
            egui::ImeEvent::Commit("done".to_owned()),
            egui::ImeEvent::Disabled,
        ] {
            let mut integration = headless_integration();
            let mut app = RecordingApp::default();
            let mut child = InputOwner::new(child_id());
            let mut root = InputOwner::new(ViewportId::ROOT);
            let field = Arc::new(Mutex::new(TextField {
                request_initial_focus: true,
                ..Default::default()
            }));
            let child_field = Arc::clone(&field);
            let callback = move |ui: &mut egui::Ui| child_field.lock().unwrap().show(ui);

            integration
                .update(&mut app, Some(&callback), child.take())
                .drop_without_applying_deltas();
            child.state.events = vec![
                key(Key::A, true),
                Event::ModifiersChanged(Modifiers::SHIFT),
                Event::Ime(egui::ImeEvent::Preedit {
                    text: "preedit".to_owned(),
                    active_range_chars: None,
                }),
            ];
            integration
                .update(&mut app, Some(&callback), child.take())
                .drop_without_applying_deltas();
            {
                let field = field.lock().unwrap();
                assert_eq!(field.text, "preedit");
                assert!(field.focused);
                assert!(field.composing);
                assert!(field.keys_down.contains(&Key::A));
                assert!(field.modifiers.shift);
            }

            let terminal_events = vec![
                key(Key::A, false),
                Event::WindowFocused(false),
                Event::ModifiersChanged(Modifiers::NONE),
                Event::Ime(terminal.clone()),
            ];
            child.state.events = terminal_events.clone();
            let hooks_before_skip = app.hooks.len();
            for _ in 0..2 {
                assert!(child.take_if(false).is_none());
            }
            assert_eq!(
                app.hooks.len(),
                hooks_before_skip,
                "skips cannot invoke hook"
            );
            root.state.events = vec![text("root-only")];
            integration
                .update(&mut app, None, root.take())
                .drop_without_applying_deltas();
            assert_eq!(app.root_passes, vec![vec![text("root-only")]]);
            assert!(egui_ime::ime_input_active_in_viewport(
                &integration.egui_ctx,
                child_id()
            ));

            child
                .state
                .events
                .extend([Event::WindowFocused(true), text("new")]);
            let mut expected_hook_events = terminal_events;
            expected_hook_events.extend([Event::WindowFocused(true), text("new")]);
            integration
                .update(&mut app, Some(&callback), child.take_if(true).unwrap())
                .drop_without_applying_deltas();
            assert_eq!(
                app.hooks.last().unwrap(),
                &(child_id(), expected_hook_events)
            );
            {
                let field = field.lock().unwrap();
                assert!(
                    field.keys_down.is_empty(),
                    "late release must end held keys"
                );
                assert_eq!(field.modifiers, Modifiers::NONE);
                assert!(!field.composing, "terminal event must end composition");
                assert!(
                    field.focused,
                    "window focus changes preserve TextEdit focus"
                );
                assert_eq!(
                    field.text,
                    if matches!(terminal, egui::ImeEvent::Commit(_)) {
                        "donenew"
                    } else {
                        "new"
                    }
                );
            }

            integration
                .update(&mut app, Some(&callback), child.take())
                .drop_without_applying_deltas();
            assert!(field.lock().unwrap().events.is_empty());
            assert!(!egui_ime::ime_input_active_in_viewport(
                &integration.egui_ctx,
                child_id()
            ));
        }
    }

    #[derive(Default)]
    struct MinimizedOutputField {
        text: String,
        events: Vec<Event>,
        info: Option<egui::ViewportInfo>,
        texture: Option<egui::TextureId>,
    }

    impl MinimizedOutputField {
        fn show(&mut self, ui: &mut egui::Ui) {
            let id = ui.ctx().viewport_id();
            self.events = ui.input(|i| i.events.clone());
            self.info = Some(ui.input(|i| i.viewport().clone()));
            let field_id = egui::Id::unique(("minimized-output-field", id));
            ui.memory_mut(|memory| memory.request_focus(field_id));
            ui.add(egui::TextEdit::singleline(&mut self.text).id(field_id));
            ui.ctx().copy_text(format!("output-{id:?}"));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Title(format!("title-{id:?}")));
            let texture = ui.ctx().load_texture(
                "minimized-output-texture",
                egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
                egui::TextureOptions::LINEAR,
            );
            self.texture = Some(texture.id());
            // This FullOutput must deliver both allocation and retirement even
            // though there will be no surface paint.
            drop(texture);
        }
    }

    #[derive(Default)]
    struct MinimizedOutputApp {
        logic_calls: usize,
        ui_calls: usize,
        root: MinimizedOutputField,
        hooks: Vec<(ViewportId, Vec<Event>)>,
    }

    impl epi::App for MinimizedOutputApp {
        fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut RawInput) {
            self.hooks.push((raw.viewport_id, raw.events.clone()));
        }

        fn logic(&mut self, _ctx: &egui::Context, _frame: &mut epi::Frame) {
            self.logic_calls += 1;
        }

        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut epi::Frame) {
            self.ui_calls += 1;
            self.root.show(ui);
            ui.ctx()
                .show_viewport_deferred(child_id(), ViewportBuilder::default(), |_, _| {});
        }
    }

    fn minimized_input(id: ViewportId, events: Vec<Event>) -> RawInput {
        let mut raw = input(id, events);
        for info in raw.viewports.values_mut() {
            info.minimized = Some(true);
            info.occluded = Some(false);
        }
        raw
    }

    fn check_minimized_output(
        output: &egui::FullOutput,
        id: ViewportId,
        field: &MinimizedOutputField,
    ) {
        let info = field.info.as_ref().unwrap();
        assert_eq!(info.minimized, Some(true));
        assert_eq!(info.visible(), Some(false), "UI cannot falsify visibility");
        assert!(
            output.platform_output.ime.is_some(),
            "real TextEdit emits IME output"
        );
        assert!(output.platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == &format!("output-{id:?}"))
        ));
        assert!(output.viewport_output[&id].commands.iter().any(
            |command| matches!(command, egui::ViewportCommand::Title(title) if title == &format!("title-{id:?}"))
        ));
        let texture = field.texture.unwrap();
        assert!(
            output
                .textures_delta
                .set
                .iter()
                .any(|(id, _)| *id == texture)
        );
        assert!(output.textures_delta.free.contains(&texture));
    }

    #[test]
    fn minimized_full_nonpaint_actions_feed_real_owner_state_once() {
        use super::super::frame_output::process_non_paint_actions;

        let integration = headless_integration();
        let mut state = egui_winit::State::new(
            integration.egui_ctx.clone(),
            child_id(),
            &integration.frame,
            None,
            None,
            None,
        );
        state.egui_input_mut().events.push(key(Key::A, false));
        let mut actions = vec![
            egui_winit::ActionRequested::Cut,
            egui_winit::ActionRequested::PaintWhileHidden,
            egui_winit::ActionRequested::Copy,
        ];
        process_non_paint_actions(&mut actions, &mut state);
        process_non_paint_actions(&mut actions, &mut state);
        assert_eq!(
            state.egui_input().events,
            vec![key(Key::A, false), Event::Cut, Event::Copy]
        );
        assert!(matches!(
            actions.as_slice(),
            [egui_winit::ActionRequested::PaintWhileHidden]
        ));
    }

    #[test]
    fn minimized_full_real_integration_keeps_root_child_input_outputs_and_logic_only_boundary() {
        use super::super::{frame_output::PassDecision, winit_integration::PassMode};

        let mut integration = headless_integration();
        let mut app = MinimizedOutputApp::default();
        let mut root_pending = RawInput::default();
        let mut child_pending = RawInput::default();
        let child = Arc::new(Mutex::new(MinimizedOutputField::default()));
        let callback_child = Arc::clone(&child);
        let child_ui_calls = Arc::new(Mutex::new(0));
        let callback_calls = Arc::clone(&child_ui_calls);
        let callback = move |ui: &mut egui::Ui| {
            *callback_calls.lock().unwrap() += 1;
            callback_child.lock().unwrap().show(ui);
        };

        let root_logic_input = minimized_input(ViewportId::ROOT, vec![text("A")]);
        let decision = PassDecision::for_viewport(
            PassMode::LogicOnly,
            root_logic_input.viewport(),
            true,
            true,
            true,
        );
        assert!(!decision.run_ui && !decision.paint);
        let (_, unused) = integration.update_logic_only(&mut app, root_logic_input);
        root_pending.append(unused);
        assert_eq!(app.logic_calls, 1);
        assert_eq!(app.ui_calls, 0);
        assert_eq!(*child_ui_calls.lock().unwrap(), 0);

        // A deferred child Full does not run App::logic or consume root input.
        let child_input = take_viewport_input(&mut child_pending, || {
            minimized_input(child_id(), vec![text("B")])
        });
        let decision =
            PassDecision::for_viewport(PassMode::Full, child_input.viewport(), false, false, true);
        assert!(decision.run_ui && !decision.paint);
        let output = integration.update(&mut app, Some(&callback), child_input);
        check_minimized_output(&output, child_id(), &child.lock().unwrap());
        output.drop_without_applying_deltas();
        assert_eq!(app.logic_calls, 1);
        assert_eq!(app.ui_calls, 0);
        assert_eq!(*child_ui_calls.lock().unwrap(), 1);
        assert_eq!(child.lock().unwrap().events, vec![text("B")]);
        assert_eq!(root_pending.events, vec![text("A")]);

        let root_input = take_viewport_input(&mut root_pending, || {
            minimized_input(ViewportId::ROOT, vec![text("C")])
        });
        let decision =
            PassDecision::for_viewport(PassMode::Full, root_input.viewport(), false, false, true);
        assert!(decision.run_ui && !decision.paint);
        let output = integration.update(&mut app, None, root_input);
        check_minimized_output(&output, ViewportId::ROOT, &app.root);
        output.drop_without_applying_deltas();
        assert_eq!(app.logic_calls, 2);
        assert_eq!(app.ui_calls, 1);
        assert_eq!(app.root.events, vec![text("A"), text("C")]);
        assert_eq!(app.root.text, "AC");
        assert_eq!(child.lock().unwrap().text, "B");
        assert_eq!(
            app.hooks,
            vec![
                (ViewportId::ROOT, vec![text("A")]),
                (child_id(), vec![text("B")]),
                (ViewportId::ROOT, vec![text("A"), text("C")]),
            ]
        );
        assert!(root_pending.events.is_empty() && child_pending.events.is_empty());
        assert!(!integration.frame.native_window_visible_commit_completed());

        // The same minimized child's explicit LogicOnly decision is a skip;
        // use the production acquisition selector to keep its queued input.
        let mut child_state = minimized_input(child_id(), vec![key(Key::A, false)]);
        let decision = PassDecision::for_viewport(
            PassMode::LogicOnly,
            child_state.viewport(),
            true,
            true,
            true,
        );
        assert!(
            take_viewport_input_if(decision.run_ui, &mut child_pending, || {
                child_state.take()
            })
            .is_none()
        );
        assert_eq!(child_state.events, vec![key(Key::A, false)]);
        assert_eq!(*child_ui_calls.lock().unwrap(), 1);
        assert_eq!(app.logic_calls, 2);
    }

    #[derive(Clone, Copy)]
    enum RootCloseDecision {
        Cancel,
        Allow,
        Complete,
    }

    struct LogicCloseApp {
        decision: RootCloseDecision,
        received: Vec<bool>,
        hooks: Vec<Vec<Event>>,
    }

    impl epi::App for LogicCloseApp {
        fn ui(&mut self, _: &mut egui::Ui, _: &mut epi::Frame) {
            panic!("root Close must be decided by logic without UI");
        }

        fn raw_input_hook(&mut self, _: &egui::Context, input: &mut RawInput) {
            assert_eq!(input.viewport_id, ViewportId::ROOT);
            self.hooks.push(input.events.clone());
        }

        fn logic(&mut self, ctx: &egui::Context, frame: &mut epi::Frame) {
            assert!(!frame.native_window_visible_commit_completed());
            let close = ctx.input(|input| {
                assert_eq!(input.raw.viewport_id, ViewportId::ROOT);
                assert_eq!(input.viewport().minimized, Some(true));
                input.viewport().close_requested()
            });
            self.received.push(close);
            match self.decision {
                RootCloseDecision::Cancel if close => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
                RootCloseDecision::Complete => {
                    // Test-only completion owner: consume its terminal state
                    // once, and return the request through ordinary output.
                    self.decision = RootCloseDecision::Allow;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                RootCloseDecision::Cancel | RootCloseDecision::Allow => {}
            }
        }
    }

    /// Fake native effects only. The actual integration decides Close, the
    /// production input helper preserves pending input, and the same completion
    /// helper used by both native backends consumes the current native event.
    struct RootCloseWindow {
        info: egui::ViewportInfo,
        pending: RawInput,
        state: RawInput,
        delivered_close: usize,
        delivered_cancel: usize,
    }

    impl RootCloseWindow {
        fn new() -> Self {
            Self {
                info: egui::ViewportInfo {
                    minimized: Some(true),
                    events: vec![egui::ViewportEvent::Close],
                    ..Default::default()
                },
                pending: Default::default(),
                state: RawInput {
                    events: vec![key(Key::A, false), Event::WindowFocused(false)],
                    ..Default::default()
                },
                delivered_close: 0,
                delivered_cancel: 0,
            }
        }

        fn take(&mut self) -> RawInput {
            let mut input = take_viewport_input(&mut self.pending, || self.state.take());
            // Native acquisition installs current metadata after combining
            // pending and new input; its old Close snapshot must not win.
            input.viewports.insert(ViewportId::ROOT, self.info.clone());
            input
        }

        fn finish(&mut self, output: egui::LogicOutput, unused: RawInput) {
            finish_root_logic_input(&mut self.pending, &mut self.info, unused);
            assert!(self.info.events.is_empty());
            assert!(output.platform_output.commands.is_empty());
            for (id, commands) in output.viewport_commands {
                assert_eq!(id, ViewportId::ROOT);
                for command in commands {
                    match command {
                        egui::ViewportCommand::Close => {
                            // Native command delivery generates the next Close
                            // event, rather than directly forcing process exit.
                            self.delivered_close += 1;
                            self.info.events.push(egui::ViewportEvent::Close);
                        }
                        egui::ViewportCommand::CancelClose => self.delivered_cancel += 1,
                        other => panic!("unexpected root lifecycle command: {other:?}"),
                    }
                }
            }
        }
    }

    fn check_root_logic_close(backend: &str) {
        for cancel_first in [false, true] {
            let mut integration = headless_integration();
            let mut window = RootCloseWindow::new();
            let mut app = LogicCloseApp {
                decision: if cancel_first {
                    RootCloseDecision::Cancel
                } else {
                    RootCloseDecision::Allow
                },
                received: vec![],
                hooks: vec![],
            };
            let ordinary = window.state.events.clone();
            let (output, unused) = integration.update_logic_only(&mut app, window.take());
            // Check immediately after logic, before native effects or any UI
            // could supply a late CancelClose.
            assert_eq!(integration.should_close(), !cancel_first, "{backend}");
            assert_eq!(app.received, [true]);
            window.finish(output, unused);
            assert_eq!(window.delivered_cancel, usize::from(cancel_first));
            assert_eq!(window.delivered_close, 0);

            let (output, unused) = integration.update_logic_only(&mut app, window.take());
            assert_eq!(integration.should_close(), !cancel_first, "{backend}");
            assert_eq!(app.received, [true, false]);
            window.finish(output, unused);

            if cancel_first {
                app.decision = RootCloseDecision::Complete;
                let (output, unused) = integration.update_logic_only(&mut app, window.take());
                assert!(
                    !integration.should_close(),
                    "a command is not an incoming Close"
                );
                assert_eq!(app.received, [true, false, false]);
                window.finish(output, unused);
                assert_eq!(window.delivered_close, 1);

                let (output, unused) = integration.update_logic_only(&mut app, window.take());
                assert!(
                    integration.should_close(),
                    "{backend}: resumed Close must exit"
                );
                assert_eq!(app.received, [true, false, false, true]);
                window.finish(output, unused);

                // An extra headless probe observes no replay. The real native
                // loop would already have returned its CloseRequested result.
                let (output, unused) = integration.update_logic_only(&mut app, window.take());
                assert!(integration.should_close());
                assert_eq!(app.received, [true, false, false, true, false]);
                window.finish(output, unused);
                assert_eq!(window.delivered_close, 1);
                assert_eq!(window.delivered_cancel, 1);
            }
            assert!(app.hooks.iter().all(|events| events == &ordinary));
            assert_eq!(window.pending.events, ordinary);
            assert!(window.info.events.is_empty());
            assert!(!integration.frame.native_window_visible_commit_completed());
            assert!(integration.pending_full_output.textures_delta.is_empty());
        }
    }

    #[cfg(feature = "wgpu_no_default_features")]
    #[test]
    fn minimized_full_root_logic_only_close_wgpu() {
        check_root_logic_close("wgpu");
    }

    #[cfg(feature = "glow")]
    #[test]
    fn minimized_full_root_logic_only_close_glow() {
        check_root_logic_close("glow");
    }
}
