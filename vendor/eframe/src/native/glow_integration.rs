//! Note that this file contains code very similar to [`super::wgpu_integration`].
//! When making changes to one you often also want to apply it to the other.
//!
//! This is also very complex code, and not very pretty.
//! There is a bunch of improvements we could do,
//! like removing a bunch of `unwraps`.

#![expect(clippy::undocumented_unsafe_blocks)]
#![expect(clippy::unwrap_used)]

use core::{cell::RefCell, num::NonZeroU32};
use std::{rc::Rc, sync::Arc, time::Instant};

use egui_winit::ActionRequested;
use glutin::{
    config::GlConfig as _,
    context::NotCurrentGlContext as _,
    display::GetGlDisplay as _,
    prelude::{GlDisplay as _, PossiblyCurrentGlContext as _},
    surface::GlSurface as _,
};
use raw_window_handle::HasWindowHandle as _;
use winit::{
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    window::{Window, WindowId},
};

use ahash::HashMap;
use egui::{
    DeferredViewportUiCallback, ImmediateViewport, OrderedViewportIdMap, ViewportBuilder,
    ViewportClass, ViewportId, ViewportIdPair, ViewportInfo, ViewportOutput,
};
#[cfg(feature = "accesskit")]
use egui_winit::accesskit_winit;

use super::{
    epi_integration, event_loop_context,
    frame_output::{
        ImmediateViewportBackend, PassDecision, WinitImmediateOutputTarget, WinitOutputTarget,
        finish_main_viewport_output, run_immediate_viewport,
    },
    viewport_lifecycle::{
        ChildCloseBackend, consume_close_in_input, dispatch_child_close_requests,
    },
    winit_integration::{EventResult, PassMode, UserEvent, WinitApp, create_egui_context},
};
use crate::epaint::textures::TexturesDelta;
use crate::{
    App, AppCreator, CreationContext, NativeOptions, Result, Storage,
    native::{epi_integration::EpiIntegration, winit_integration::sleep_if_invisible_or_minimized},
};

// ----------------------------------------------------------------------------
// Types:

pub struct GlowWinitApp<'app> {
    repaint_proxy: Arc<egui::mutex::Mutex<EventLoopProxy<UserEvent>>>,
    app_name: String,
    native_options: NativeOptions,
    running: Option<GlowWinitRunning<'app>>,

    // Note that since this `AppCreator` is FnOnce we are currently unable to support
    // re-initializing the `GlowWinitRunning` state on Android if the application
    // suspends and resumes.
    app_creator: Option<AppCreator<'app>>,

    /// An optional pre-existing egui context. If `Some`, it is used instead of
    /// creating a new one via [`create_egui_context`]. Taken during initialization.
    egui_ctx: Option<egui::Context>,
}

/// State that is initialized when the application is first starts running via
/// a Resumed event. On Android this ensures that any graphics state is only
/// initialized once the application has an associated `SurfaceView`.
struct GlowWinitRunning<'app> {
    integration: EpiIntegration,
    app: Box<dyn 'app + App>,

    // These needs to be shared with the immediate viewport renderer, hence the Rc/Arc/RefCells:
    glutin: Rc<RefCell<GlutinWindowContext>>,

    // NOTE: one painter shared by all viewports.
    painter: Rc<RefCell<egui_glow::Painter>>,

    /// Any not yet applied deltas for this app.
    pending_deltas: Rc<RefCell<TexturesDelta>>,
}

impl Drop for GlowWinitRunning<'_> {
    fn drop(&mut self) {
        // Avoid debug panic when dropping unapplied deltas on teardown
        self.pending_deltas.borrow_mut().clear();
    }
}

/// This struct will contain both persistent and temporary glutin state.
///
/// Platform Quirks:
/// * Microsoft Windows: requires that we create a window before opengl context.
/// * Android: window and surface should be destroyed when we receive a suspend event. recreate on resume event.
///
/// winit guarantees that we will get a Resumed event on startup on all platforms.
/// * Before Resumed event: `gl_config`, `gl_context` can be created at any time. on windows, a window must be created to get `gl_context`.
/// * Resumed: `gl_surface` will be created here. `window` will be re-created here for android.
/// * Suspended: on android, we drop window + surface.  on other platforms, we don't get Suspended event.
///
/// The setup is divided between the `new` fn and `on_resume` fn. we can just assume that `on_resume` is a continuation of
/// `new` fn on all platforms. only on android, do we get multiple resumed events because app can be suspended.
struct GlutinWindowContext {
    egui_ctx: egui::Context,

    swap_interval: glutin::surface::SwapInterval,
    gl_config: glutin::config::Config,

    max_texture_side: Option<usize>,

    /// Passed on to each [`egui_winit::State`]. See [`NativeOptions::clipboard_shortcuts`].
    clipboard_shortcuts: bool,

    current_gl_context: Option<glutin::context::PossiblyCurrentContext>,
    not_current_gl_context: Option<glutin::context::NotCurrentContext>,

    /// The viewport whose `gl_surface` the context was last made current with.
    ///
    /// `None` if the context is not current, or if that surface has since been dropped.
    ///
    /// We track this ourselves instead of only asking glutin's `Surface::is_current`,
    /// because on Windows (WGL) that only checks if the _context_ is current, ignoring the surface.
    /// With several viewports sharing one context, that gives the wrong answer.
    /// See <https://github.com/emilk/egui/issues/4289>.
    current_viewport: Option<ViewportId>,

    viewports: OrderedViewportIdMap<Viewport>,
    viewport_from_window: HashMap<WindowId, ViewportId>,
    window_from_viewport: OrderedViewportIdMap<WindowId>,

    focused_viewport: Option<ViewportId>,
}

struct Viewport {
    ids: ViewportIdPair,
    registration: Option<egui::ViewportRegistration>,
    parent_registration: Option<egui::ViewportRegistration>,
    class: ViewportClass,
    builder: ViewportBuilder,
    deferred_commands: Vec<egui::viewport::ViewportCommand>,
    info: ViewportInfo,
    actions_requested: Vec<egui_winit::ActionRequested>,

    /// Input not consumed by a UI pass yet, owned only by this viewport.
    pending_raw_input: egui::RawInput,

    /// The user-callback that shows the ui.
    /// None for immediate viewports.
    viewport_ui_cb: Option<Arc<DeferredViewportUiCallback>>,

    // These three live and die together.
    // TODO(emilk): clump them together into one struct!
    gl_surface: Option<glutin::surface::Surface<glutin::surface::WindowSurface>>,
    window: Option<Arc<Window>>,
    egui_winit: Option<egui_winit::State>,
}

impl Viewport {
    /// Forward raw mouse motion to egui.
    ///
    /// Returns the window to repaint if the event was used.
    fn on_mouse_motion(&mut self, delta: (f64, f64)) -> Option<winit::window::WindowId> {
        let window = self.window.as_ref()?;
        let egui_winit = self.egui_winit.as_mut()?;
        egui_winit.on_mouse_motion(delta).then(|| window.id())
    }

    /// Apply the commands, or defer them until we have a window.
    fn process_commands(
        &mut self,
        egui_ctx: &egui::Context,
        mut commands: Vec<egui::ViewportCommand>,
    ) {
        self.deferred_commands.append(&mut commands);

        if let Some(window) = &self.window {
            egui_winit::process_viewport_commands(
                egui_ctx,
                &mut self.info,
                core::mem::take(&mut self.deferred_commands),
                window,
                &mut self.actions_requested,
            );
        }
    }
}

// ----------------------------------------------------------------------------

fn create_window_for_viewport(
    egui_ctx: &egui::Context,
    gl_config: &glutin::config::Config,
    builder: &ViewportBuilder,
    event_loop: &ActiveEventLoop,
) -> Result<(Arc<Window>, ViewportInfo)> {
    let window_attributes = egui_winit::apply_monitor_to_window_attributes(
        egui_winit::create_winit_window_attributes(egui_ctx, builder.clone()),
        builder,
        event_loop,
    );
    if window_attributes.transparent()
        && gl_config.supports_transparency() == Some(false)
        && !cfg!(target_os = "windows")
    {
        log::error!("Cannot create transparent window: the GL config does not support it");
    }

    let window = cfg_select! {
        target_os = "windows" => {
            if window_attributes.transparent() {
                // Preserve explicitly requested transparency for both root and child windows.
                // Some GL paths report no transparency support although composition works.
                event_loop.create_window(window_attributes)?
            } else {
                glutin_winit::finalize_window(event_loop, window_attributes, gl_config)?
            }
        }
        _ => {
            // Keep the normal platform-specific finalization path elsewhere.
            glutin_winit::finalize_window(event_loop, window_attributes, gl_config)?
        }
    };
    egui_winit::apply_viewport_builder_to_window(egui_ctx, &window, builder);

    let mut viewport_info = ViewportInfo::default();
    egui_winit::update_viewport_info(&mut viewport_info, egui_ctx, &window, true);
    Ok((Arc::new(window), viewport_info))
}

// ----------------------------------------------------------------------------

impl<'app> GlowWinitApp<'app> {
    pub fn new(
        event_loop: &EventLoop<UserEvent>,
        app_name: &str,
        native_options: NativeOptions,
        egui_ctx: Option<egui::Context>,
        app_creator: AppCreator<'app>,
    ) -> Self {
        profiling::function_scope!();
        Self {
            repaint_proxy: Arc::new(egui::mutex::Mutex::new(event_loop.create_proxy())),
            app_name: app_name.to_owned(),
            native_options,
            running: None,
            app_creator: Some(app_creator),
            egui_ctx,
        }
    }

    #[expect(unsafe_code)]
    fn create_glutin_windowed_context(
        egui_ctx: &egui::Context,
        event_loop: &ActiveEventLoop,
        storage: Option<&dyn Storage>,
        native_options: &mut NativeOptions,
    ) -> Result<(GlutinWindowContext, egui_glow::Painter)> {
        profiling::function_scope!();
        let mut window_settings = epi_integration::load_window_settings(storage);

        let winit_window_builder = epi_integration::viewport_builder(
            egui_ctx.zoom_factor(),
            event_loop,
            native_options,
            &mut window_settings,
        )
        .with_visible(false); // Start hidden until we render the first frame to fix white flash on startup (https://github.com/emilk/egui/pull/3631)

        let mut glutin_window_context = unsafe {
            GlutinWindowContext::new(egui_ctx, winit_window_builder, native_options, event_loop)?
        };

        // Creates the window - must come before we create our glow context
        glutin_window_context.initialize_window(ViewportId::ROOT, event_loop)?;

        {
            let viewport = &glutin_window_context.viewports[&ViewportId::ROOT];
            let window = viewport.window.as_ref().unwrap(); // Can't fail - we just called `initialize_all_viewports`
            epi_integration::apply_window_settings(window, window_settings);
        }

        let gl = unsafe {
            profiling::scope!("glow::Context::from_loader_function");
            Arc::new(glow::Context::from_loader_function(|s| {
                let s = std::ffi::CString::new(s)
                    .expect("failed to construct C string from string for gl proc address");

                glutin_window_context.get_proc_address(&s)
            }))
        };

        let painter = egui_glow::Painter::new(
            gl,
            "",
            native_options.glow_options.shader_version,
            native_options.dithering,
        )?;

        Ok((glutin_window_context, painter))
    }

    fn init_run_state(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) -> Result<&mut GlowWinitRunning<'app>> {
        profiling::function_scope!();

        let storage = if let Some(file) = &self.native_options.persistence_path {
            epi_integration::create_storage_with_file(file)
        } else {
            epi_integration::create_storage(
                self.native_options
                    .viewport
                    .app_id
                    .as_ref()
                    .unwrap_or(&self.app_name),
            )
        };

        let egui_ctx = self
            .egui_ctx
            .take()
            .unwrap_or_else(|| create_egui_context(storage.as_deref()));

        let (mut glutin, painter) = Self::create_glutin_windowed_context(
            &egui_ctx,
            event_loop,
            storage.as_deref(),
            &mut self.native_options,
        )?;
        let gl = Arc::clone(painter.gl());

        let max_texture_side = painter.max_texture_side();
        glutin.max_texture_side = Some(max_texture_side);
        for viewport in glutin.viewports.values_mut() {
            if let Some(egui_winit) = viewport.egui_winit.as_mut() {
                egui_winit.set_max_texture_side(max_texture_side);
            }
        }

        let painter = Rc::new(RefCell::new(painter));

        let integration = EpiIntegration::new(
            egui_ctx,
            &glutin.window(ViewportId::ROOT),
            &self.app_name,
            &self.native_options,
            storage,
            Some(Arc::clone(&gl)),
            Some(Box::new({
                let painter = Rc::clone(&painter);
                move |native| painter.borrow_mut().register_native_texture(native)
            })),
            #[cfg(feature = "wgpu_no_default_features")]
            None,
        );

        {
            let event_loop_proxy = Arc::clone(&self.repaint_proxy);
            integration
                .egui_ctx
                .set_request_repaint_callback(move |info| {
                    log::trace!("request_repaint_callback: {info:?}");
                    let when = Instant::now() + info.delay;
                    let cumulative_pass_nr = info.current_cumulative_pass_nr;
                    event_loop_proxy
                        .lock()
                        .send_event(UserEvent::RequestRepaint {
                            viewport_id: info.viewport_id,
                            when,
                            cumulative_pass_nr,
                        })
                        .ok();
                });
        }

        #[cfg(feature = "accesskit")]
        {
            let event_loop_proxy = self.repaint_proxy.lock().clone();
            let viewport = glutin.viewports.get_mut(&ViewportId::ROOT).unwrap(); // we always have a root
            if let Viewport {
                window: Some(window),
                egui_winit: Some(egui_winit),
                ..
            } = viewport
            {
                egui_winit.init_accesskit(event_loop, window, event_loop_proxy);
            }
        }

        if self
            .native_options
            .viewport
            .mouse_passthrough
            .unwrap_or(false)
            && let Err(err) = glutin.window(ViewportId::ROOT).set_cursor_hittest(false)
        {
            log::warn!("set_cursor_hittest(false) failed: {err}");
        }

        let app_creator = core::mem::take(&mut self.app_creator)
            .expect("Single-use AppCreator has unexpectedly already been taken");

        crate::maybe_attach_inspection_plugin(&integration.egui_ctx, Some(self.app_name.clone()));

        let app: Box<dyn 'app + App> = {
            // Use latest raw_window_handle for eframe compatibility
            use raw_window_handle::{HasDisplayHandle as _, HasWindowHandle as _};

            let gl_config = glutin.gl_config.clone();
            let get_proc_address = move |addr: &_| gl_config.display().get_proc_address(addr);
            let window = glutin.window(ViewportId::ROOT);
            let cc = CreationContext {
                egui_ctx: integration.egui_ctx.clone(),
                integration_info: integration.frame.info().clone(),
                storage: integration.frame.storage(),
                gl: Some(gl),
                get_proc_address: Some(Arc::new(get_proc_address)),
                #[cfg(feature = "wgpu_no_default_features")]
                wgpu_render_state: None,
                window: Some(Arc::clone(&window)),
                raw_display_handle: window.display_handle().map(|h| h.as_raw()),
                raw_window_handle: window.window_handle().map(|h| h.as_raw()),
            };
            profiling::scope!("app_creator");
            app_creator(&cc).map_err(crate::Error::AppCreation)?
        };

        let glutin = Rc::new(RefCell::new(glutin));
        let pending_deltas = Rc::new(RefCell::new(TexturesDelta::default()));

        {
            // Create weak pointers so that we don't keep
            // state alive for too long.
            let glutin = Rc::downgrade(&glutin);
            let painter = Rc::downgrade(&painter);
            let pending_deltas = Rc::downgrade(&pending_deltas);
            let beginning = integration.beginning;

            egui::Context::set_immediate_viewport_renderer(move |egui_ctx, immediate_viewport| {
                if let (Some(glutin), Some(painter), Some(pending_deltas)) = (
                    glutin.upgrade(),
                    painter.upgrade(),
                    pending_deltas.upgrade(),
                ) {
                    render_immediate_viewport(
                        &mut NativeGlowImmediateBackend {
                            egui_ctx,
                            glutin: &glutin,
                            painter: &painter,
                            pending_deltas: &pending_deltas,
                            beginning,
                        },
                        immediate_viewport,
                    );
                } else {
                    log::warn!("render_sync_callback called after window closed");
                }
            });
        }

        Ok(self.running.insert(GlowWinitRunning {
            integration,
            app,
            glutin,
            painter,
            pending_deltas,
        }))
    }
}

impl WinitApp for GlowWinitApp<'_> {
    fn egui_ctx(&self) -> Option<&egui::Context> {
        self.running.as_ref().map(|r| &r.integration.egui_ctx)
    }

    fn window(&self, window_id: WindowId) -> Option<Arc<Window>> {
        let running = self.running.as_ref()?;
        let glutin = running.glutin.borrow();
        let viewport_id = *glutin.viewport_from_window.get(&window_id)?;
        if let Some(viewport) = glutin.viewports.get(&viewport_id) {
            viewport.window.clone()
        } else {
            None
        }
    }

    fn window_id_from_viewport_id(&self, id: ViewportId) -> Option<WindowId> {
        self.running
            .as_ref()?
            .glutin
            .borrow()
            .window_from_viewport
            .get(&id)
            .copied()
    }

    fn save(&mut self) {
        log::debug!("WinitApp::save called");
        if let Some(running) = self.running.as_mut() {
            profiling::function_scope!();

            // This is used because of the "save on suspend" logic on Android. Once the application is suspended, there is no window associated to it, which was causing panics when `.window().expect()` was used.
            let window_opt = running.glutin.borrow().window_opt(ViewportId::ROOT);

            running
                .integration
                .save(running.app.as_mut(), window_opt.as_deref());
        }
    }

    fn save_and_destroy(&mut self) {
        if let Some(mut running) = self.running.take() {
            profiling::function_scope!();

            let ctx = running.integration.egui_ctx.clone();
            super::viewport_lifecycle::finish_pending_retirements(
                &ctx,
                &mut GlowChildCloseBackend {
                    glutin: &running.glutin,
                    painter: &running.painter,
                    pending_deltas: &running.pending_deltas,
                },
            );
            ctx.on_exit();
            running.integration.save(
                running.app.as_mut(),
                Some(&running.glutin.borrow().window(ViewportId::ROOT)),
            );
            running.app.on_exit(Some(running.painter.borrow().gl()));
            super::viewport_lifecycle::finish_pending_retirements(
                &ctx,
                &mut GlowChildCloseBackend {
                    glutin: &running.glutin,
                    painter: &running.painter,
                    pending_deltas: &running.pending_deltas,
                },
            );
            running.painter.borrow_mut().destroy();
        }
    }

    fn run_ui_and_paint(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        mode: PassMode,
    ) -> Result<EventResult> {
        if let Some(running) = &mut self.running {
            running.run_ui_and_paint(event_loop, window_id, mode)
        } else {
            Ok(EventResult::Wait)
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) -> crate::Result<EventResult> {
        log::debug!("Event::Resumed");

        let running = if let Some(running) = &mut self.running {
            // Not the first resume event. Create all outstanding windows.
            running
                .glutin
                .borrow_mut()
                .initialize_all_windows(event_loop);
            running
        } else {
            // First resume event. Create our root window etc.
            self.init_run_state(event_loop)?
        };
        let window_id = running.glutin.borrow().window_from_viewport[&ViewportId::ROOT];
        Ok(EventResult::RepaintNow(window_id))
    }

    fn suspended(&mut self, _: &ActiveEventLoop) -> crate::Result<EventResult> {
        if let Some(running) = &mut self.running {
            running.glutin.borrow_mut().on_suspend()?;
        }
        Ok(EventResult::Save)
    }

    fn device_event(
        &mut self,
        _: &ActiveEventLoop,
        _: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) -> crate::Result<EventResult> {
        if let winit::event::DeviceEvent::MouseMotion { delta } = event
            && let Some(running) = &mut self.running
        {
            // `MouseMotion` is not associated with any window, so we deliver it to
            // the viewport that has the pointer (or an ongoing drag), preferring the focused one.
            // We don't require that viewport to have focus, since another viewport may have stolen it
            // (e.g. on Wayland, where focus-stealing prevention can refuse `ViewportCommand::Focus`).
            let mut glutin = running.glutin.borrow_mut();
            let focused_viewport = glutin.focused_viewport;
            let viewports = &mut glutin.viewports;
            if let Some(window_id) = focused_viewport
                .and_then(|id| viewports.get_mut(&id))
                .and_then(|viewport| viewport.on_mouse_motion(delta))
                .or_else(|| {
                    viewports
                        .values_mut()
                        .find_map(|viewport| viewport.on_mouse_motion(delta))
                })
            {
                return Ok(EventResult::RepaintNext(window_id));
            }
        }

        Ok(EventResult::Wait)
    }

    fn window_event(
        &mut self,
        _: &ActiveEventLoop,
        window_id: WindowId,
        event: winit::event::WindowEvent,
    ) -> Result<EventResult> {
        if let Some(running) = &mut self.running {
            Ok(running.on_window_event(window_id, &event))
        } else {
            Ok(EventResult::Exit)
        }
    }

    #[cfg(feature = "accesskit")]
    fn on_accesskit_event(&mut self, event: accesskit_winit::Event) -> crate::Result<EventResult> {
        use super::winit_integration;

        if let Some(running) = &self.running {
            let mut glutin = running.glutin.borrow_mut();
            if let Some(viewport_id) = glutin.viewport_from_window.get(&event.window_id).copied()
                && let Some(viewport) = glutin.viewports.get_mut(&viewport_id)
                && let Some(egui_winit) = &mut viewport.egui_winit
            {
                return Ok(winit_integration::on_accesskit_window_event(
                    egui_winit,
                    event.window_id,
                    &event.window_event,
                ));
            }
        }

        Ok(EventResult::Wait)
    }
}

impl GlowWinitRunning<'_> {
    fn dispatch_child_close_requests(&mut self) {
        dispatch_child_close_requests(
            &self.integration.egui_ctx,
            self.app.as_mut(),
            &mut GlowChildCloseBackend {
                glutin: &self.glutin,
                painter: &self.painter,
                pending_deltas: &self.pending_deltas,
            },
        );
    }

    fn run_ui_and_paint(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        mode: PassMode,
    ) -> Result<EventResult> {
        profiling::function_scope!();

        // Close is a lifecycle request, and must precede input acquisition even
        // for an immediate viewport or a LogicOnly/otherwise skipped child.
        self.dispatch_child_close_requests();

        let Some(viewport_id) = self
            .glutin
            .borrow()
            .viewport_from_window
            .get(&window_id)
            .copied()
        else {
            return Ok(EventResult::Wait);
        };

        profiling::finish_frame!();

        let mut frame_timer = crate::stopwatch::Stopwatch::new();
        frame_timer.start();

        {
            let glutin = self.glutin.borrow();
            let viewport = &glutin.viewports[&viewport_id];
            let is_immediate = viewport.viewport_ui_cb.is_none();
            if is_immediate && viewport_id != ViewportId::ROOT {
                // This will only happen if this is an immediate viewport.
                // That means that the viewport cannot be rendered by itself and needs his parent to be rendered.
                if let Some(parent_viewport) = glutin.viewports.get(&viewport.ids.parent)
                    && let Some(window) = parent_viewport.window.as_ref()
                {
                    return Ok(EventResult::RepaintNext(window.id()));
                }
                return Ok(EventResult::Wait);
            }
        }

        let (raw_input, viewport_ui_cb, paint, show_ui) = {
            let mut glutin = self.glutin.borrow_mut();
            let egui_ctx = glutin.egui_ctx.clone();
            let Some(viewport) = glutin.viewports.get_mut(&viewport_id) else {
                return Ok(EventResult::Wait);
            };
            let Some(window) = viewport.window.as_ref() else {
                return Ok(EventResult::Wait);
            };
            egui_winit::update_viewport_info(&mut viewport.info, &egui_ctx, window, false);

            let viewport_ui_cb = viewport.viewport_ui_cb.clone();
            let info = viewport.info.clone();
            let actions_want_paint = viewport
                .actions_requested
                .iter()
                .any(egui_winit::ActionRequested::wants_paint);
            let decision = PassDecision::for_viewport(
                mode,
                &info,
                is_viewport_or_descendant_visible(&glutin.viewports, viewport_id),
                actions_want_paint,
                cfg!(target_os = "windows"),
            );
            let show_ui = decision.run_ui;

            let viewport = glutin.viewports.get_mut(&viewport_id).unwrap();
            let Some(window) = viewport.window.as_ref() else {
                return Ok(EventResult::Wait);
            };
            let Some(egui_winit) = viewport.egui_winit.as_mut() else {
                return Ok(EventResult::Wait);
            };
            // A skipped child must retain its input in State. Only root logic consumes
            // an input batch without a UI pass, and returns it to this viewport below.
            let raw_input = epi_integration::take_viewport_input_if(
                show_ui || viewport_id == ViewportId::ROOT,
                &mut viewport.pending_raw_input,
                || egui_winit.take_egui_input(window),
            );

            let raw_input = raw_input.map(|mut raw_input| {
                self.integration.pre_update();
                raw_input.time = Some(self.integration.beginning.elapsed().as_secs_f64());
                raw_input.viewports = glutin
                    .viewports
                    .iter()
                    .map(|(id, viewport)| (*id, viewport.info.clone()))
                    .collect();
                raw_input
            });

            (raw_input, viewport_ui_cb, decision.paint, show_ui)
        };

        if !show_ui {
            // Nothing will be shown, so we run no egui pass at all.
            // That way all ui state is left untouched, and is still there
            // when this viewport becomes visible again.
            let is_root_viewport = viewport_ui_cb.is_none();
            if is_root_viewport {
                // The app logic keeps ticking, so it can e.g. ask to be shown again:
                let (mut logic_output, unused_input) = self.integration.update_logic_only(
                    self.app.as_mut(),
                    raw_input.expect("root logic always acquires its own input"),
                );

                {
                    let mut backend = GlowChildCloseBackend {
                        glutin: &self.glutin,
                        painter: &self.painter,
                        pending_deltas: &self.pending_deltas,
                    };
                    super::viewport_lifecycle::finish_logic_output(
                        &self.integration.egui_ctx,
                        &mut backend,
                        &mut logic_output,
                    );
                }
                let egui::LogicOutput {
                    platform_output,
                    viewport_commands,
                } = logic_output;
                let mut glutin = self.glutin.borrow_mut();
                if let Some(viewport) = glutin.viewports.get_mut(&viewport_id) {
                    epi_integration::finish_root_logic_input(
                        &mut viewport.pending_raw_input,
                        &mut viewport.info,
                        unused_input,
                    );
                    if let Some(window) = viewport.window.clone()
                        && let Some(egui_winit) = viewport.egui_winit.as_mut()
                    {
                        egui_winit.handle_platform_output_with_event_loop(
                            &window,
                            event_loop,
                            platform_output,
                        );
                    }
                }
                for (id, commands) in viewport_commands {
                    if let Some(viewport) = glutin.viewports.get_mut(&id) {
                        viewport.process_commands(&self.integration.egui_ctx, commands);
                    }
                }
            }

            sleep_if_invisible_or_minimized(
                self.glutin
                    .borrow()
                    .viewports
                    .get(&viewport_id)
                    .and_then(|viewport| viewport.window.as_deref()),
            );

            // Logic commands can close a child without producing a UI pass.
            self.dispatch_child_close_requests();

            return Ok(if self.integration.should_close() {
                EventResult::CloseRequested
            } else {
                EventResult::Wait
            });
        }

        let raw_input = raw_input.expect("a UI pass always acquires its viewport input");

        // HACK: In order to get the right clear_color, the system theme needs to be set, which
        // usually only happens in the `update` call. So we call Options::begin_pass early
        // to set the right theme. Without this there would be a black flash on the first frame.
        self.integration
            .egui_ctx
            .options_mut(|opt| opt.begin_pass(&raw_input));
        let clear_color = self
            .app
            .clear_color(&self.integration.egui_ctx.global_style().visuals);

        let has_many_viewports = self.glutin.borrow().viewports.len() > 1;
        let clear_before_update = !has_many_viewports; // HACK: for some reason, an early clear doesn't "take" on Mac with multiple viewports.

        let cleared_before_update = paint && clear_before_update;
        if cleared_before_update {
            // clear before we call update, so users can paint between clear-color and egui windows:

            let mut glutin = self.glutin.borrow_mut();
            let GlutinWindowContext {
                viewports,
                current_gl_context,
                not_current_gl_context,
                current_viewport,
                ..
            } = &mut *glutin;
            let viewport = &viewports[&viewport_id];
            let Some(window) = viewport.window.as_ref() else {
                return Ok(EventResult::Wait);
            };
            let Some(gl_surface) = viewport.gl_surface.as_ref() else {
                return Ok(EventResult::Wait);
            };

            let screen_size_in_pixels: [u32; 2] = window.inner_size().into();

            {
                frame_timer.pause();
                change_gl_context(
                    current_gl_context,
                    not_current_gl_context,
                    current_viewport,
                    viewport_id,
                    gl_surface,
                );
                frame_timer.resume();
            }

            self.painter
                .borrow()
                .clear(screen_size_in_pixels, clear_color);
        }

        // ------------------------------------------------------------
        // The update function, which could call immediate viewports,
        // so make sure we don't hold any locks here required by the immediate viewports rendeer.

        let _discard_texture_scope = enter_discard_texture_delivery(
            &self.integration.egui_ctx,
            &self.glutin,
            &self.painter,
            &self.pending_deltas,
        );
        let mut full_output =
            self.integration
                .update(self.app.as_mut(), viewport_ui_cb.as_deref(), raw_input);

        super::viewport_lifecycle::append_texture_delta(
            &mut self.pending_deltas.borrow_mut(),
            core::mem::take(&mut full_output.textures_delta),
        );
        super::viewport_lifecycle::finish_full_output(
            &self.integration.egui_ctx,
            &mut GlowChildCloseBackend {
                glutin: &self.glutin,
                painter: &self.painter,
                pending_deltas: &self.pending_deltas,
            },
            &mut full_output,
        );

        // ------------------------------------------------------------

        let Self {
            integration,
            app,
            glutin: glutin_cell,
            painter: painter_cell,
            pending_deltas: pending_deltas_cell,
            ..
        } = self;

        let mut glutin = glutin_cell.borrow_mut();
        let mut painter = painter_cell.borrow_mut();
        let mut pending_deltas = pending_deltas_cell.borrow_mut();

        let egui::FullOutput {
            platform_output,
            textures_delta,
            shapes,
            pixels_per_point,
            mut viewport_output,
        } = full_output;
        super::viewport_lifecycle::append_texture_delta(&mut pending_deltas, textures_delta);

        let window = glutin
            .viewports
            .get(&viewport_id)
            .and_then(|viewport| viewport.window.clone());
        let paint = if let Some(viewport) = glutin.viewports.get_mut(&viewport_id) {
            viewport.info.events.clear(); // they should have been processed
            if let Some(egui_winit) = viewport.egui_winit.as_mut()
                && let Some(window) = &window
                && let Some(output) = viewport_output.get_mut(&viewport_id)
            {
                // Current-UI actions and platform/IME output complete before
                // painting is decided, including when the surface is hidden.
                finish_main_viewport_output(
                    platform_output,
                    &mut output.commands,
                    &mut viewport.actions_requested,
                    &mut WinitOutputTarget {
                        state: egui_winit,
                        window,
                        event_loop,
                    },
                );
            }
            PassDecision::for_viewport(
                mode,
                &viewport.info,
                false,
                viewport
                    .actions_requested
                    .iter()
                    .any(ActionRequested::wants_paint),
                cfg!(target_os = "windows"),
            )
            .paint
                && window.is_some()
                && viewport.egui_winit.is_some()
                && viewport.gl_surface.is_some()
        } else {
            false
        };

        if paint {
            let GlutinWindowContext {
                viewports,
                current_gl_context,
                not_current_gl_context,
                current_viewport,
                ..
            } = &mut *glutin;
            let viewport = viewports.get_mut(&viewport_id).unwrap();
            let window = window.as_ref().unwrap();
            let gl_surface = viewport.gl_surface.as_ref().unwrap();
            let clipped_primitives = integration.egui_ctx.tessellate(shapes, pixels_per_point);

            {
                // We may need to switch contexts again, because of immediate viewports:
                frame_timer.pause();
                change_gl_context(
                    current_gl_context,
                    not_current_gl_context,
                    current_viewport,
                    viewport_id,
                    gl_surface,
                );
                frame_timer.resume();
            }

            let screen_size_in_pixels: [u32; 2] = window.inner_size().into();

            if !cleared_before_update {
                painter.clear(screen_size_in_pixels, clear_color);
            }

            painter.paint_and_update_textures(
                screen_size_in_pixels,
                pixels_per_point,
                &clipped_primitives,
                &mut pending_deltas,
            );

            {
                let mut screenshot_callbacks = Vec::new();
                for action in viewport.actions_requested.drain(..) {
                    match action {
                        ActionRequested::Screenshot(callback) => {
                            screenshot_callbacks.push(callback);
                        }
                        ActionRequested::PaintWhileHidden => {
                            // Painting this frame is all it asked for.
                        }
                        ActionRequested::Cut | ActionRequested::Copy | ActionRequested::Paste => {
                            unreachable!("clipboard actions are consumed before painting");
                        }
                    }
                }

                if !screenshot_callbacks.is_empty() {
                    let screenshot = Arc::new(painter.read_screen_rgba(screen_size_in_pixels));
                    for callback in screenshot_callbacks {
                        callback.complete(Arc::clone(&screenshot));
                    }
                }

                integration.post_rendering(window);
            }

            {
                // vsync - don't count as frame-time:
                frame_timer.pause();
                profiling::scope!("swap_buffers");
                let context = current_gl_context.as_ref().ok_or_else(|| {
                    egui_glow::PainterError::from(
                        "failed to get current context to swap buffers".to_owned(),
                    )
                })?;

                // On Wayland this makes winit hold back the next `RedrawRequested` until the
                // compositor sends a frame callback. That is our frame pacing there, since we
                // swap with interval 0 on Wayland (see `GlutinWindowContext::new`).
                // egui-wgpu does the same. A no-op on other platforms.
                window.pre_present_notify();
                gl_surface.swap_buffers(context)?;
                frame_timer.resume();
            }

            // give it time to settle:
            #[cfg(feature = "__screenshot")]
            if integration.egui_ctx.cumulative_pass_nr() == 2
                && let Ok(path) = std::env::var("EFRAME_SCREENSHOT_TO")
            {
                save_screenshot_and_exit(&path, &painter, screen_size_in_pixels);
            }
        } else if !pending_deltas.is_empty() {
            // A UI pass can generate textures without painting any surface.
            // Apply them now on a valid shared context; if no surface exists,
            // the existing renderer owner retains them until one is available.
            finish_glow_texture_delta(
                glutin.make_resource_context_current(viewport_id),
                &mut *painter,
                &mut pending_deltas,
            );
        }

        glutin.handle_viewport_output(event_loop, &integration.egui_ctx, &viewport_output);

        integration.report_frame_time(frame_timer.total_time_sec()); // don't count auto-save time as part of regular frame time

        integration.maybe_autosave(
            app.as_mut(),
            (viewport_id == ViewportId::ROOT)
                .then_some(window.as_deref())
                .flatten(),
        );

        sleep_if_invisible_or_minimized(window.as_deref());

        // Nested immediate output is now merged into this outer output and all
        // commands/GC and shared texture ownership have completed. App hooks
        // and Context retirement must run outside every renderer/native borrow.
        drop(window);
        drop(pending_deltas);
        drop(painter);
        drop(glutin);
        dispatch_child_close_requests(
            &integration.egui_ctx,
            app.as_mut(),
            &mut GlowChildCloseBackend {
                glutin: glutin_cell,
                painter: &*painter_cell,
                pending_deltas: &*pending_deltas_cell,
            },
        );

        if integration.should_close() {
            Ok(EventResult::CloseRequested)
        } else {
            Ok(EventResult::Wait)
        }
    }

    fn on_window_event(
        &mut self,
        window_id: WindowId,
        event: &winit::event::WindowEvent,
    ) -> EventResult {
        let result = self.on_window_event_inner(window_id, event);
        // on_window_event_inner has released GlutinWindowContext and State.
        self.dispatch_child_close_requests();
        result
    }

    fn on_window_event_inner(
        &mut self,
        window_id: WindowId,
        event: &winit::event::WindowEvent,
    ) -> EventResult {
        let mut glutin = self.glutin.borrow_mut();
        let viewport_id = glutin.viewport_from_window.get(&window_id).copied();

        // On Windows, if a window is resized by the user, it should repaint synchronously, inside the
        // event handler.
        //
        // If this is not done, the compositor will assume that the window does not want to redraw,
        // and continue ahead.
        //
        // In eframe's case, that causes the window to rapidly flicker, as it struggles to deliver
        // new frames to the compositor in time.
        //
        // The flickering is technically glutin or glow's fault, but we should be responding properly
        // to resizes anyway, as doing so avoids dropping frames.
        //
        // See: https://github.com/emilk/egui/issues/903
        let mut repaint_asap = false;

        match event {
            winit::event::WindowEvent::Focused(focused) => {
                let focused = if cfg!(target_os = "macos")
                    && let Some(viewport_id) = viewport_id
                    && let Some(viewport) = glutin.viewports.get(&viewport_id)
                    && let Some(window) = &viewport.window
                {
                    // TODO(emilk): remove this work-around once we update winit
                    // https://github.com/rust-windowing/winit/issues/4371
                    // https://github.com/emilk/egui/issues/7588
                    window.has_focus()
                } else {
                    *focused
                };

                glutin.focused_viewport = focused.then_some(viewport_id).flatten();
            }

            // The reply to `Frame::request_activation_token`. Hold it for the app
            // to take, and repaint: a token is short-lived and single-use, so an
            // idle app must not have to wait for an unrelated event to notice it.
            winit::event::WindowEvent::ActivationTokenDone { token, .. } => {
                self.integration.frame.activation_token = Some(token.clone().into_raw());
                if let Some(viewport_id) = viewport_id {
                    self.integration.egui_ctx.request_repaint_of(viewport_id);
                } else {
                    self.integration.egui_ctx.request_repaint();
                }
            }

            winit::event::WindowEvent::Resized(physical_size) => {
                // The event can be stale: winit buffers events emitted while we create and set up
                // the window (e.g. maximize it), so it can arrive after the window has changed size again.
                let physical_size = &viewport_id
                    .and_then(|id| glutin.viewports.get(&id))
                    .and_then(|viewport| viewport.window.as_ref())
                    .map_or(*physical_size, |window| window.inner_size());

                // Resize with 0 width and height is used by winit to signal a minimize event on Windows.
                // See: https://github.com/rust-windowing/winit/issues/208
                // This solves an issue where the app would panic when minimizing on Windows.
                if 0 < physical_size.width
                    && 0 < physical_size.height
                    && let Some(viewport_id) = viewport_id
                {
                    repaint_asap = true;
                    glutin.resize(viewport_id, *physical_size);
                }
            }

            winit::event::WindowEvent::Occluded(is_occluded) => {
                if let Some(viewport_id) = viewport_id
                    && let Some(viewport) = glutin.viewports.get_mut(&viewport_id)
                {
                    viewport.info.occluded = Some(*is_occluded);
                }
            }

            winit::event::WindowEvent::CloseRequested => {
                if viewport_id == Some(ViewportId::ROOT) && self.integration.should_close() {
                    log::debug!(
                        "Received WindowEvent::CloseRequested for main viewport - shutting down."
                    );
                    return EventResult::CloseRequested;
                }

                log::debug!("Received WindowEvent::CloseRequested for viewport {viewport_id:?}");

                if let Some(viewport_id) = viewport_id
                    && let Some(viewport) = glutin.viewports.get_mut(&viewport_id)
                {
                    // Tell viewport it should close:
                    viewport.info.events.push(egui::ViewportEvent::Close);

                    // We may need to repaint both us and our parent to close the window,
                    // and perhaps twice (once to notice the close-event, once again to enforce it).
                    // `request_repaint_of` does a double-repaint though:
                    self.integration.egui_ctx.request_repaint_of(viewport_id);
                    self.integration
                        .egui_ctx
                        .request_repaint_of(viewport.ids.parent);
                }
            }
            _ => {}
        }

        if self.integration.should_close() {
            return EventResult::CloseRequested;
        }

        let mut event_response = egui_winit::EventResponse {
            consumed: false,
            repaint: false,
        };
        if let Some(viewport_id) = viewport_id {
            if let Some(viewport) = glutin.viewports.get_mut(&viewport_id) {
                if let (Some(window), Some(egui_winit)) =
                    (&viewport.window, &mut viewport.egui_winit)
                {
                    event_response = self.integration.on_window_event(window, egui_winit, event);
                }
            } else {
                log::trace!("Ignoring event: no viewport for {viewport_id:?}");
            }
        } else {
            log::trace!("Ignoring event: no viewport_id");
        }

        if event_response.repaint {
            if repaint_asap {
                EventResult::RepaintNow(window_id)
            } else {
                EventResult::RepaintNext(window_id)
            }
        } else {
            EventResult::Wait
        }
    }
}

/// Short native borrows only: neither the App hook nor Context retirement may
/// hold the RefCell borrowed by the immediate viewport renderer.
struct GlowChildCloseBackend<'a> {
    glutin: &'a RefCell<GlutinWindowContext>,
    painter: &'a RefCell<egui_glow::Painter>,
    pending_deltas: &'a RefCell<TexturesDelta>,
}

impl ChildCloseBackend for GlowChildCloseBackend<'_> {
    fn close_requests(&self) -> Vec<ViewportId> {
        glow_child_close_requests(&self.glutin.borrow().viewports)
    }

    fn consume_close(&mut self, id: ViewportId) {
        consume_glow_child_close(&mut self.glutin.borrow_mut().viewports, id);
    }

    fn registration(
        &self,
        ctx: &egui::Context,
        id: ViewportId,
    ) -> Option<egui::ViewportRegistration> {
        self.glutin
            .borrow()
            .viewports
            .get(&id)
            .and_then(|vp| vp.registration)
            .or_else(|| ctx.viewport_registration(id))
    }

    fn retire(&mut self, ctx: &egui::Context, approval: &egui::ApprovedViewportRetirement) {
        let (approval, retired) = super::viewport_lifecycle::native_retirement_ids(
            ctx,
            approval,
            self.glutin
                .borrow()
                .viewports
                .iter()
                .map(|(id, vp)| (*id, vp.registration, vp.parent_registration)),
        );
        retire_glow_native_resources(self.glutin, &retired);
        super::viewport_lifecycle::finish_context_retirement(ctx, &approval);
    }

    fn gc(&mut self, ctx: &egui::Context) {
        let retired = self
            .glutin
            .borrow()
            .viewports
            .iter()
            .filter_map(|(id, vp)| {
                (*id != ViewportId::ROOT
                    && vp.registration.is_some_and(|registration| {
                        ctx.viewport_registration(*id) != Some(registration)
                    }))
                .then_some(*id)
            })
            .collect();
        retire_glow_native_resources(self.glutin, &retired);
        let ime_ids: Vec<_> = retired
            .into_iter()
            .filter(|id| ctx.viewport_registration(*id).is_none())
            .collect();
        egui_ime::retire_viewport_ime_policy(ctx, &ime_ids);
    }

    fn collect_retirement_delta(&mut self, ctx: &egui::Context) {
        super::viewport_lifecycle::append_texture_delta(
            &mut self.pending_deltas.borrow_mut(),
            ctx.take_viewport_retirement_texture_delta(),
        );
    }

    fn finish_retirement(&mut self, ctx: &egui::Context) {
        super::viewport_lifecycle::finish_retirement_texture_delta(
            ctx,
            &mut self.pending_deltas.borrow_mut(),
            |delta| {
                let current = self
                    .glutin
                    .borrow_mut()
                    .make_resource_context_current(ViewportId::ROOT);
                finish_glow_texture_delta(current, &mut *self.painter.borrow_mut(), delta);
            },
        );
    }
}

fn enter_discard_texture_delivery(
    ctx: &egui::Context,
    glutin: &Rc<RefCell<GlutinWindowContext>>,
    painter: &Rc<RefCell<egui_glow::Painter>>,
    pending: &Rc<RefCell<TexturesDelta>>,
) -> impl Drop + use<> {
    let glutin = Rc::downgrade(glutin);
    let painter = Rc::downgrade(painter);
    let pending = Rc::downgrade(pending);
    ctx.enter_discard_texture_delivery(move |delta| {
        let glutin = glutin
            .upgrade()
            .expect("native Full GL context must outlive its scope");
        let painter = painter
            .upgrade()
            .expect("native Full painter must outlive its scope");
        let pending = pending
            .upgrade()
            .expect("native Full pending owner must outlive its scope");
        super::viewport_lifecycle::append_texture_delta(&mut pending.borrow_mut(), delta);
        if !pending.borrow().is_empty() {
            let current = glutin
                .borrow_mut()
                .make_resource_context_current(ViewportId::ROOT);
            finish_glow_texture_delta(
                current,
                &mut *painter.borrow_mut(),
                &mut pending.borrow_mut(),
            );
        }
    })
}

fn replace_stale_glow_viewport(
    glutin: &RefCell<GlutinWindowContext>,
    pending: &RefCell<TexturesDelta>,
    ctx: &egui::Context,
    id: ViewportId,
) {
    let stale = super::viewport_lifecycle::stale_native_subtree(
        ctx,
        id,
        glutin
            .borrow()
            .viewports
            .iter()
            .map(|(id, vp)| (*id, vp.registration, vp.parent_registration)),
    );
    if !stale.is_empty() {
        retire_glow_native_resources(glutin, &stale);
        super::viewport_lifecycle::append_texture_delta(
            &mut pending.borrow_mut(),
            ctx.tex_manager().write().take_delta(),
        );
    }
}

fn retire_glow_native_resources(
    glutin: &RefCell<GlutinWindowContext>,
    retired: &egui::ViewportIdSet,
) {
    let removed = {
        let mut glutin = glutin.borrow_mut();
        let order = super::viewport_lifecycle::child_first_ids(
            retired,
            glutin.viewports.iter().map(|(id, vp)| (*id, vp.ids.parent)),
        );
        let removed: Vec<_> = order
            .into_iter()
            .filter_map(|id| glutin.viewports.remove(&id))
            .collect();
        glutin.retire_viewport_resources(retired);
        removed
    };
    drop(removed);
}

fn glow_child_close_requests<Owner: std::borrow::Borrow<Viewport>>(
    viewports: &OrderedViewportIdMap<Owner>,
) -> Vec<ViewportId> {
    viewports
        .iter()
        .filter_map(|(id, vp)| {
            let vp = vp.borrow();
            (*id != ViewportId::ROOT
                && (vp.info.close_requested()
                    || vp.deferred_commands.contains(&egui::ViewportCommand::Close)))
            .then_some(*id)
        })
        .collect()
}

fn consume_glow_child_close<Owner: std::borrow::BorrowMut<Viewport>>(
    viewports: &mut OrderedViewportIdMap<Owner>,
    id: ViewportId,
) {
    for (owner, vp) in viewports {
        let vp = vp.borrow_mut();
        if *owner == id {
            vp.info
                .events
                .retain(|event| !matches!(event, egui::ViewportEvent::Close));
            vp.deferred_commands
                .retain(|command| !matches!(command, egui::ViewportCommand::Close));
        }
        // Every snapshot can mention the target; only its Close flag is
        // consumed. Ordinary events, metadata and sibling flags stay intact.
        consume_close_in_input(&mut vp.pending_raw_input, id);
        if let Some(state) = &mut vp.egui_winit {
            consume_close_in_input(state.egui_input_mut(), id);
        }
    }
}

#[cfg(test)]
fn glow_viewport_subtree(
    root: ViewportId,
    parents: impl Iterator<Item = (ViewportId, ViewportId)>,
) -> egui::ViewportIdSet {
    let parents: Vec<_> = parents.collect();
    let mut retired = egui::ViewportIdSet::from_iter([root]);
    loop {
        let count = retired.len();
        for (id, parent) in &parents {
            if *id != ViewportId::ROOT && retired.contains(parent) {
                retired.insert(*id);
            }
        }
        if retired.len() == count {
            return retired;
        }
    }
}

/// Common resource-owner cleanup for normal output GC and approved child
/// retirement. Generic owners let headless tests observe real map removals and
/// destruction without constructing a GL context or a native window.
fn retire_glow_viewport_resources<Owner, WindowKey: Eq + std::hash::Hash>(
    viewports: &mut OrderedViewportIdMap<Owner>,
    viewport_from_window: &mut HashMap<WindowKey, ViewportId>,
    window_from_viewport: &mut OrderedViewportIdMap<WindowKey>,
    current_viewport: &mut Option<ViewportId>,
    focused_viewport: &mut Option<ViewportId>,
    retired: &egui::ViewportIdSet,
) {
    if current_viewport.is_some_and(|id| retired.contains(&id)) {
        // The shared GL context survives; its cached surface identity does not.
        *current_viewport = None;
    }
    if focused_viewport.is_some_and(|id| retired.contains(&id)) {
        *focused_viewport = None;
    }
    viewport_from_window.retain(|_, id| !retired.contains(id));
    window_from_viewport.retain(|id, _| !retired.contains(id));
    // Drop each target's State, pending input, surface and window together.
    viewports.retain(|id, _| !retired.contains(id));
}

/// Resource updates use a surviving shared-context surface, independent of
/// whether the viewport that produced them still exists or will be painted.
fn select_glow_resource_viewport(
    preferred: ViewportId,
    current: Option<ViewportId>,
    available: impl Iterator<Item = ViewportId>,
) -> Option<ViewportId> {
    available.max_by_key(|id| (*id == preferred, Some(*id) == current))
}

trait GlowTexturePainter {
    fn set_texture(&mut self, id: egui::TextureId, delta: &egui::epaint::ImageDelta);
    fn free_texture(&mut self, id: egui::TextureId);
}

impl GlowTexturePainter for egui_glow::Painter {
    fn set_texture(&mut self, id: egui::TextureId, delta: &egui::epaint::ImageDelta) {
        Self::set_texture(self, id, delta);
    }

    fn free_texture(&mut self, id: egui::TextureId) {
        Self::free_texture(self, id);
    }
}

/// Complete a non-paint FullOutput's resource work on a current GL context.
/// Without one, keep the existing renderer-owned delta intact.
fn finish_glow_texture_delta(
    has_current_context: bool,
    painter: &mut impl GlowTexturePainter,
    pending: &mut TexturesDelta,
) {
    if !has_current_context {
        return;
    }
    #[expect(clippy::iter_over_hash_type)] // Different texture IDs are independent.
    for (id, image_deltas) in pending.set.drain() {
        for delta in image_deltas {
            painter.set_texture(id, &delta);
        }
    }
    #[expect(clippy::iter_over_hash_type)] // Free order does not matter.
    for id in pending.free.drain() {
        painter.free_texture(id);
    }
}

fn change_gl_context(
    current_gl_context: &mut Option<glutin::context::PossiblyCurrentContext>,
    not_current_gl_context: &mut Option<glutin::context::NotCurrentContext>,
    current_viewport: &mut Option<ViewportId>,
    viewport_id: ViewportId,
    gl_surface: &glutin::surface::Surface<glutin::surface::WindowSurface>,
) {
    profiling::function_scope!();

    // Early-out to save a lot of time: switching context is expensive,
    // especially on Windows (https://github.com/emilk/egui/issues/4173).
    //
    // We cannot rely only on `gl_surface.is_current`, because on Windows it ignores the surface
    // and only checks if the context is current. With several viewports sharing one context
    // (or after the surface the context was current on has been dropped), it gives the wrong
    // answer: https://github.com/emilk/egui/issues/4289
    // So we keep track of which viewport the context is current on ourselves.
    if *current_viewport == Some(viewport_id)
        && let Some(current_gl_context) = current_gl_context
    {
        profiling::scope!("is_current");
        if gl_surface.is_current(current_gl_context) {
            return;
        }
    }

    let not_current = if let Some(not_current_context) = not_current_gl_context.take() {
        not_current_context
    } else {
        profiling::scope!("make_not_current");
        *current_viewport = None;
        current_gl_context
            .take()
            .unwrap()
            .make_not_current()
            .unwrap()
    };

    profiling::scope!("make_current");
    *current_gl_context = Some(not_current.make_current(gl_surface).unwrap());
    *current_viewport = Some(viewport_id);
}

impl GlutinWindowContext {
    fn make_resource_context_current(&mut self, preferred: ViewportId) -> bool {
        let viewport_id = select_glow_resource_viewport(
            preferred,
            self.current_viewport,
            self.viewports
                .iter()
                .filter_map(|(id, viewport)| viewport.gl_surface.as_ref().map(|_| *id)),
        );
        let Some(viewport_id) = viewport_id else {
            return false;
        };
        if self.current_gl_context.is_none() && self.not_current_gl_context.is_none() {
            return false;
        }
        change_gl_context(
            &mut self.current_gl_context,
            &mut self.not_current_gl_context,
            &mut self.current_viewport,
            viewport_id,
            self.viewports[&viewport_id].gl_surface.as_ref().unwrap(),
        );
        true
    }

    #[expect(unsafe_code)]
    unsafe fn new(
        egui_ctx: &egui::Context,
        viewport_builder: ViewportBuilder,
        native_options: &NativeOptions,
        event_loop: &ActiveEventLoop,
    ) -> Result<Self> {
        profiling::function_scope!();

        // There is a lot of complexity with opengl creation,
        // so prefer extensive logging to get all the help we can to debug issues.

        use glutin::prelude::*;
        // convert native options to glutin options
        let hardware_acceleration = match native_options.glow_options.hardware_acceleration {
            egui_glow::HardwareAcceleration::Required => Some(true),
            egui_glow::HardwareAcceleration::Preferred => None,
            egui_glow::HardwareAcceleration::Off => Some(false),
        };
        // On Wayland, winit paces `RedrawRequested` by the compositor's frame callbacks
        // (see `pre_present_notify`), which is what vsync means there. A blocking swap would add
        // nothing but a way to hang: a compositor may never present a hidden window's buffer.
        // See <https://github.com/emilk/egui/issues/5136>.
        let is_wayland = {
            use raw_window_handle::{HasDisplayHandle as _, RawDisplayHandle};
            event_loop
                .display_handle()
                .is_ok_and(|handle| matches!(handle.as_raw(), RawDisplayHandle::Wayland(_)))
        };
        let swap_interval = if native_options.glow_options.vsync && !is_wayland {
            glutin::surface::SwapInterval::Wait(NonZeroU32::MIN)
        } else {
            glutin::surface::SwapInterval::DontWait
        };
        /*  opengl setup flow goes like this:
            1. we create a configuration for opengl "Display" / "Config" creation
            2. choose between special extensions like glx or egl or wgl and use them to create config/display
            3. opengl context configuration
            4. opengl context creation
        */
        // start building config for gl display
        let config_template_builder = glutin::config::ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(hardware_acceleration)
            .with_depth_size(native_options.depth_buffer)
            .with_stencil_size(native_options.stencil_buffer)
            .with_transparency(native_options.viewport.transparent.unwrap_or(false));
        // we don't know if multi sampling option is set. so, check if its more than 0.
        let config_template_builder = if native_options.multisampling > 0 {
            config_template_builder.with_multisampling(
                native_options
                    .multisampling
                    .try_into()
                    .expect("failed to fit multisamples option of native_options into u8"),
            )
        } else {
            config_template_builder
        };

        log::debug!("trying to create glutin Display with config: {config_template_builder:?}");

        // Create GL display. This may probably create a window too on most platforms. Definitely on `MS windows`. Never on Android.
        let display_builder = glutin_winit::DisplayBuilder::new()
            // we might want to expose this option to users in the future. maybe using an env var or using native_options.
            //
            // The justification for FallbackEgl over PreferEgl is at https://github.com/emilk/egui/pull/2526#issuecomment-1400229576 .
            .with_preference(glutin_winit::ApiPreference::FallbackEgl)
            .with_window_attributes(Some(egui_winit::apply_monitor_to_window_attributes(
                egui_winit::create_winit_window_attributes(egui_ctx, viewport_builder.clone()),
                &viewport_builder,
                event_loop,
            )));

        let (window, gl_config) = {
            profiling::scope!("DisplayBuilder::build");

            display_builder
                .build(
                    event_loop,
                    config_template_builder.clone(),
                    |mut config_iterator| {
                        let config = config_iterator.next().expect(
                            "failed to find a matching configuration for creating glutin config",
                        );
                        log::debug!(
                            "using the first config from config picker closure. config: {config:?}"
                        );
                        config
                    },
                )
                .map_err(|e| crate::Error::NoGlutinConfigs(config_template_builder.build(), e))?
        };
        if let Some(window) = &window {
            egui_winit::apply_viewport_builder_to_window(egui_ctx, window, &viewport_builder);
        }

        let gl_display = gl_config.display();
        log::debug!(
            "successfully created GL Display with version: {} and supported features: {:?}",
            gl_display.version_string(),
            gl_display.supported_features()
        );
        let glutin_raw_window_handle = window.as_ref().map(|w| {
            w.window_handle()
                .expect("Failed to get window handle")
                .as_raw()
        });
        log::debug!("creating gl context using raw window handle: {glutin_raw_window_handle:?}");

        let [default_attributes, fallback_attributes @ ..] =
            context_attributes_to_try(glutin_raw_window_handle);
        let gl_context = create_first_context(
            default_attributes,
            fallback_attributes,
            |context_attributes| unsafe {
                profiling::scope!("create_context");
                gl_config
                    .display()
                    .create_context(&gl_config, context_attributes)
            },
        )?;
        let not_current_gl_context = Some(gl_context);

        let mut viewport_from_window = HashMap::default();
        let mut window_from_viewport = OrderedViewportIdMap::default();
        let mut viewport_info = ViewportInfo::default();
        if let Some(window) = &window {
            viewport_from_window.insert(window.id(), ViewportId::ROOT);
            window_from_viewport.insert(ViewportId::ROOT, window.id());
            egui_winit::update_viewport_info(&mut viewport_info, egui_ctx, window, true);

            // Tell egui right away about native_pixels_per_point etc,
            // so that the app knows about it during app creation:
            let pixels_per_point = egui_winit::pixels_per_point(egui_ctx, window);

            egui_ctx.input_mut(|i| {
                i.raw
                    .viewports
                    .insert(ViewportId::ROOT, viewport_info.clone());

                i.pixels_per_point = pixels_per_point;
            });
        }

        let mut viewports = OrderedViewportIdMap::default();
        viewports.insert(
            ViewportId::ROOT,
            Viewport {
                ids: ViewportIdPair::ROOT,
                registration: egui_ctx.viewport_registration(ViewportId::ROOT),
                parent_registration: None,
                class: ViewportClass::Root,
                builder: viewport_builder,
                deferred_commands: vec![],
                info: viewport_info,
                actions_requested: Default::default(),
                pending_raw_input: Default::default(),
                viewport_ui_cb: None,
                gl_surface: None,
                window: window.map(Arc::new),
                egui_winit: None,
            },
        );

        // the fun part with opengl gl is that we never know whether there is an error. the context creation might have failed, but
        // it could keep working until we try to make surface current or swap buffers or something else. future glutin improvements might
        // help us start from scratch again if we fail context creation and go back to preferEgl or try with different config etc..
        // https://github.com/emilk/egui/pull/2541#issuecomment-1370767582

        let mut slf = Self {
            egui_ctx: egui_ctx.clone(),
            swap_interval,
            gl_config,
            current_gl_context: None,
            not_current_gl_context,
            current_viewport: None,
            viewports,
            viewport_from_window,
            max_texture_side: None,
            clipboard_shortcuts: native_options.clipboard_shortcuts,
            window_from_viewport,
            focused_viewport: Some(ViewportId::ROOT),
        };

        slf.initialize_window(ViewportId::ROOT, event_loop)?;

        Ok(slf)
    }

    /// Create a surface, window, and winit integration for all viewports lacking any of that.
    ///
    /// Errors will be logged.
    fn initialize_all_windows(&mut self, event_loop: &ActiveEventLoop) {
        profiling::function_scope!();

        let viewports: Vec<ViewportId> = self.viewports.keys().copied().collect();

        for viewport_id in viewports {
            if let Err(err) = self.initialize_window(viewport_id, event_loop) {
                log::error!("Failed to initialize a window for viewport {viewport_id:?}: {err}");
            }
        }
    }

    /// Create a surface, window, and winit integration for the viewport, if missing.
    #[expect(unsafe_code)]
    pub(crate) fn initialize_window(
        &mut self,
        viewport_id: ViewportId,
        event_loop: &ActiveEventLoop,
    ) -> Result {
        profiling::function_scope!();

        let viewport = self
            .viewports
            .get_mut(&viewport_id)
            .expect("viewport doesn't exist");

        let window = if let Some(window) = &mut viewport.window {
            window
        } else {
            log::debug!("Creating a window for viewport {viewport_id:?}");
            let (window, viewport_info) = create_window_for_viewport(
                &self.egui_ctx,
                &self.gl_config,
                &viewport.builder,
                event_loop,
            )?;
            viewport.info = viewport_info;
            viewport.window.insert(window)
        };

        viewport.egui_winit.get_or_insert_with(|| {
            log::debug!("Initializing egui_winit for viewport {viewport_id:?}");
            let mut egui_winit = egui_winit::State::new(
                self.egui_ctx.clone(),
                viewport_id,
                event_loop,
                Some(window.scale_factor() as f32),
                event_loop.system_theme(),
                self.max_texture_side,
            );
            egui_winit.set_clipboard_shortcuts(self.clipboard_shortcuts);
            egui_winit
        });

        if viewport.gl_surface.is_none() {
            log::debug!("Creating a gl_surface for viewport {viewport_id:?}");

            // surface attributes
            let (width_px, height_px): (u32, u32) = window.inner_size().into();
            let width_px = NonZeroU32::new(width_px).unwrap_or(NonZeroU32::MIN);
            let height_px = NonZeroU32::new(height_px).unwrap_or(NonZeroU32::MIN);
            let surface_attributes = {
                glutin::surface::SurfaceAttributesBuilder::<glutin::surface::WindowSurface>::new()
                    .build(
                        window
                            .window_handle()
                            .expect("Failed to get display handle")
                            .as_raw(),
                        width_px,
                        height_px,
                    )
            };

            log::trace!("creating surface with attributes: {surface_attributes:?}");
            let gl_surface = unsafe {
                self.gl_config
                    .display()
                    .create_window_surface(&self.gl_config, &surface_attributes)?
            };

            log::trace!("surface created successfully: {gl_surface:?}. making context current");

            let not_current_gl_context =
                if let Some(not_current_context) = self.not_current_gl_context.take() {
                    not_current_context
                } else {
                    self.current_viewport = None;
                    self.current_gl_context
                        .take()
                        .unwrap()
                        .make_not_current()
                        .unwrap()
                };
            let current_gl_context = not_current_gl_context.make_current(&gl_surface)?;
            self.current_viewport = Some(viewport_id);

            // try setting swap interval. but its not absolutely necessary, so don't panic on failure.
            log::trace!("made context current. setting swap interval for surface");
            if let Err(err) = gl_surface.set_swap_interval(&current_gl_context, self.swap_interval)
            {
                log::warn!("Failed to set swap interval due to error: {err}");
            }

            // we will reach this point only once in most platforms except android.
            // create window/surface/make context current once and just use them forever.

            viewport.gl_surface = Some(gl_surface);

            self.current_gl_context = Some(current_gl_context);
        }

        self.viewport_from_window.insert(window.id(), viewport_id);
        self.window_from_viewport.insert(viewport_id, window.id());

        Ok(())
    }

    /// only applies for android. but we basically drop surface + window and make context not current
    fn on_suspend(&mut self) -> Result {
        log::debug!("received suspend event. dropping window and surface");
        for viewport in self.viewports.values_mut() {
            viewport.gl_surface = None;
            viewport.window = None;
        }
        self.current_viewport = None;
        if let Some(current) = self.current_gl_context.take() {
            log::debug!("context is current, so making it non-current");
            self.not_current_gl_context = Some(current.make_not_current()?);
        } else {
            log::debug!("context is already not current??? could be duplicate suspend event");
        }
        Ok(())
    }

    fn viewport(&self, viewport_id: ViewportId) -> &Viewport {
        self.viewports
            .get(&viewport_id)
            .expect("viewport doesn't exist")
    }

    fn window_opt(&self, viewport_id: ViewportId) -> Option<Arc<Window>> {
        self.viewport(viewport_id).window.clone()
    }

    fn window(&self, viewport_id: ViewportId) -> Arc<Window> {
        self.window_opt(viewport_id)
            .expect("winit window doesn't exist")
    }

    fn resize(&mut self, viewport_id: ViewportId, physical_size: winit::dpi::PhysicalSize<u32>) {
        let width_px = NonZeroU32::new(physical_size.width).unwrap_or(NonZeroU32::MIN);
        let height_px = NonZeroU32::new(physical_size.height).unwrap_or(NonZeroU32::MIN);

        if let Some(viewport) = self.viewports.get(&viewport_id)
            && let Some(gl_surface) = &viewport.gl_surface
        {
            change_gl_context(
                &mut self.current_gl_context,
                &mut self.not_current_gl_context,
                &mut self.current_viewport,
                viewport_id,
                gl_surface,
            );
            gl_surface.resize(
                self.current_gl_context
                    .as_ref()
                    .expect("failed to get current context to resize surface"),
                width_px,
                height_px,
            );
        }
    }

    fn get_proc_address(&self, addr: &core::ffi::CStr) -> *const core::ffi::c_void {
        self.gl_config.display().get_proc_address(addr)
    }

    fn retire_viewport_resources(&mut self, retired: &egui::ViewportIdSet) {
        retire_glow_viewport_resources(
            &mut self.viewports,
            &mut self.viewport_from_window,
            &mut self.window_from_viewport,
            &mut self.current_viewport,
            &mut self.focused_viewport,
            retired,
        );
    }

    fn handle_viewport_output(
        &mut self,
        event_loop: &ActiveEventLoop,
        egui_ctx: &egui::Context,
        viewport_output: &OrderedViewportIdMap<ViewportOutput>,
    ) {
        profiling::function_scope!();

        for (
            viewport_id,
            ViewportOutput {
                registration,
                parent_registration,
                parent,
                class,
                builder,
                viewport_ui_cb,
                commands,
                repaint_delay: _, // ignored - we listened to the repaint callback instead
            },
        ) in viewport_output.clone()
        {
            if egui_ctx.viewport_registration(viewport_id) != Some(registration)
                || self
                    .viewports
                    .get(&viewport_id)
                    .is_some_and(|vp| vp.registration.is_some_and(|native| native != registration))
            {
                continue; // replacement belongs to outer cleanup or immediate prepare
            }
            let ids = ViewportIdPair::from_self_and_parent(viewport_id, parent);

            let viewport = initialize_or_update_viewport(
                &mut self.viewports,
                &mut self.current_viewport,
                ids,
                class,
                builder,
                viewport_ui_cb,
            );

            viewport.registration = Some(registration);
            viewport.parent_registration = parent_registration;
            let old_inner_size = viewport.window.as_ref().map(|window| window.inner_size());

            viewport.process_commands(egui_ctx, commands);

            // For Wayland : https://github.com/emilk/egui/issues/4196
            if cfg!(target_os = "linux")
                && let Some(window) = &viewport.window
                && let Some(old_inner_size) = old_inner_size
            {
                let new_inner_size = window.inner_size();
                if new_inner_size != old_inner_size {
                    self.resize(viewport_id, new_inner_size);
                }
            }
        }

        // Create windows for any new viewports:
        self.initialize_all_windows(event_loop);
    }
}

fn initialize_or_update_viewport<'a>(
    viewports: &'a mut OrderedViewportIdMap<Viewport>,
    current_viewport: &mut Option<ViewportId>,
    ids: ViewportIdPair,
    class: ViewportClass,
    mut builder: ViewportBuilder,
    viewport_ui_cb: Option<Arc<dyn Fn(&mut egui::Ui) + Send + Sync>>,
) -> &'a mut Viewport {
    profiling::function_scope!();

    use std::collections::btree_map::Entry;

    if builder.icon.is_none() {
        // Inherit icon from parent
        builder.icon = viewports
            .get_mut(&ids.parent)
            .and_then(|vp| vp.builder.icon.clone());
    }

    let root_transparent = viewports
        .get(&ViewportId::ROOT)
        .and_then(|viewport| viewport.builder.transparent);

    match viewports.entry(ids.this) {
        Entry::Vacant(entry) => {
            // New viewport:
            if ids.this != ViewportId::ROOT && builder.transparent.is_none() {
                // Child viewports inherit the root setting unless they explicitly override it.
                builder.transparent = root_transparent;
            }
            log::debug!("Creating new viewport {:?} ({:?})", ids.this, builder.title);
            entry.insert(Viewport {
                ids,
                registration: None,
                parent_registration: None,
                class,
                builder,
                deferred_commands: vec![],
                info: Default::default(),
                actions_requested: Default::default(),
                pending_raw_input: Default::default(),
                viewport_ui_cb,
                window: None,
                egui_winit: None,
                gl_surface: None,
            })
        }

        Entry::Occupied(mut entry) => {
            // Patch an existing viewport:
            let viewport = entry.get_mut();

            viewport.ids.parent = ids.parent;
            viewport.class = class;
            viewport.viewport_ui_cb = viewport_ui_cb;

            let (mut delta_commands, recreate) = viewport.builder.patch(builder);

            if recreate {
                log::debug!(
                    "Recreating window for viewport {:?} ({:?})",
                    ids.this,
                    viewport.builder.title
                );
                viewport.window = None;
                if let Some(state) = viewport.egui_winit.as_mut() {
                    epi_integration::retain_viewport_input_before_state_replacement(
                        viewport.ids.this,
                        &mut viewport.pending_raw_input,
                        state,
                    );
                }
                viewport.egui_winit = None;
                viewport.gl_surface = None;
                if *current_viewport == Some(ids.this) {
                    *current_viewport = None;
                }
            }

            viewport.deferred_commands.append(&mut delta_commands);

            entry.into_mut()
        }
    }
}

/// Is this viewport, or any of its (transitive) descendant viewports, visible?
///
/// Immediate viewports are rendered inline while their parent's UI runs, so even
/// if this viewport's window is occluded or minimized we must still run its UI to
/// give any visible descendant a chance to be painted.
fn is_viewport_or_descendant_visible(
    viewports: &OrderedViewportIdMap<Viewport>,
    viewport_id: ViewportId,
) -> bool {
    let Some(viewport) = viewports.get(&viewport_id) else {
        return false;
    };
    if viewport.info.visible().unwrap_or(true) {
        return true;
    }
    viewports.values().any(|child| {
        child.ids.parent == viewport_id
            && child.ids.this != viewport_id // ROOT is its own parent; avoid self-recursion.
            && is_viewport_or_descendant_visible(viewports, child.ids.this)
    })
}

/// This is called (via a callback) by user code to render immediate viewports,
/// i.e. viewport that are directly nested inside a parent viewport.
fn render_immediate_viewport(
    backend: &mut impl ImmediateViewportBackend,
    immediate_viewport: ImmediateViewport<'_>,
) {
    profiling::function_scope!();
    run_immediate_viewport(backend, immediate_viewport);
}

struct NativeGlowImmediateBackend<'a> {
    egui_ctx: &'a egui::Context,
    glutin: &'a RefCell<GlutinWindowContext>,
    painter: &'a RefCell<egui_glow::Painter>,
    pending_deltas: &'a RefCell<TexturesDelta>,
    beginning: Instant,
}

impl ImmediateViewportBackend for NativeGlowImmediateBackend<'_> {
    fn prepare(
        &mut self,
        ids: ViewportIdPair,
        builder: ViewportBuilder,
    ) -> Option<(egui::Context, egui::RawInput)> {
        assert!(
            self.egui_ctx.has_discard_texture_delivery(),
            "native immediate requires its Full resource scope"
        );
        replace_stale_glow_viewport(self.glutin, self.pending_deltas, self.egui_ctx, ids.this);
        let mut glutin = self.glutin.borrow_mut();
        let glutin = &mut *glutin;
        initialize_or_update_viewport(
            &mut glutin.viewports,
            &mut glutin.current_viewport,
            ids,
            ViewportClass::Immediate,
            builder,
            None,
        );
        let viewport = glutin.viewports.get_mut(&ids.this)?;
        viewport.registration = self.egui_ctx.viewport_registration(ids.this);
        viewport.parent_registration = self.egui_ctx.viewport_registration(ids.parent);
        let ret = event_loop_context::with_current_event_loop(|event_loop| {
            glutin.initialize_window(ids.this, event_loop)
        });
        if let Some(Err(err)) = ret {
            log::error!(
                "Failed to initialize a window for immediate viewport {:?}: {err}",
                ids.this
            );
            return None;
        }

        let viewport = glutin.viewports.get_mut(&ids.this)?;
        let (Some(state), Some(window)) = (&mut viewport.egui_winit, &viewport.window) else {
            return None;
        };
        egui_winit::update_viewport_info(&mut viewport.info, self.egui_ctx, window, false);
        let mut input =
            epi_integration::take_viewport_input(&mut viewport.pending_raw_input, || {
                state.take_egui_input(window)
            });
        input.viewports = glutin
            .viewports
            .iter()
            .map(|(id, viewport)| (*id, viewport.info.clone()))
            .collect();
        input.time = Some(self.beginning.elapsed().as_secs_f64());
        Some((self.egui_ctx.clone(), input))
        // All native borrows end before the shared driver calls user UI.
    }

    fn accept_textures(
        &mut self,
        _ids: ViewportIdPair,
        _ctx: &egui::Context,
        textures_delta: TexturesDelta,
    ) {
        // This renderer owner survives a child's disappearance or missing surface.
        super::viewport_lifecycle::append_texture_delta(
            &mut self.pending_deltas.borrow_mut(),
            textures_delta,
        );
    }

    fn finish_output(
        &mut self,
        ids: ViewportIdPair,
        ctx: &egui::Context,
        platform: egui::PlatformOutput,
        viewport_output: &mut OrderedViewportIdMap<ViewportOutput>,
    ) -> Option<(ViewportInfo, bool)> {
        let mut glutin = self.glutin.borrow_mut();
        let viewport = glutin.viewports.get_mut(&ids.this)?;
        viewport.info.events.clear();
        let (Some(state), Some(window), Some(output)) = (
            &mut viewport.egui_winit,
            &viewport.window,
            viewport_output.get_mut(&ids.this),
        ) else {
            return None;
        };
        finish_main_viewport_output(
            platform,
            &mut output.commands,
            &mut viewport.actions_requested,
            &mut WinitImmediateOutputTarget { state, window },
        );
        egui_winit::update_viewport_info(&mut viewport.info, ctx, window, false);
        // Metadata stays truthful. A missing native surface has no paint target,
        // but its output and shared texture work have already been accepted.
        viewport.gl_surface.as_ref()?;
        Some((
            viewport.info.clone(),
            viewport
                .actions_requested
                .iter()
                .any(ActionRequested::wants_paint),
        ))
    }

    fn paint(
        &mut self,
        ids: ViewportIdPair,
        ctx: &egui::Context,
        clipped_primitives: Vec<egui::ClippedPrimitive>,
        pixels_per_point: f32,
    ) {
        let mut glutin = self.glutin.borrow_mut();
        if glutin
            .viewports
            .get(&ids.this)
            .is_none_or(|viewport| viewport.window.is_none() || viewport.gl_surface.is_none())
        {
            drop(glutin);
            self.apply_resources(ids, ctx);
            return;
        }
        let GlutinWindowContext {
            current_gl_context,
            not_current_gl_context,
            current_viewport,
            viewports,
            ..
        } = &mut *glutin;
        let viewport = viewports.get_mut(&ids.this).unwrap();
        let window = viewport.window.as_ref().unwrap();
        let gl_surface = viewport.gl_surface.as_ref().unwrap();
        let screen_size_in_pixels: [u32; 2] = window.inner_size().into();
        change_gl_context(
            current_gl_context,
            not_current_gl_context,
            current_viewport,
            ids.this,
            gl_surface,
        );
        let current_gl_context = current_gl_context.as_ref().unwrap();
        if !gl_surface.is_current(current_gl_context) {
            log::error!(
                "egui::show_viewport_immediate: viewport {:?} ({:?}) was not created on main thread.",
                viewport.ids.this,
                viewport.builder.title
            );
        }
        let mut painter = self.painter.borrow_mut();
        painter.clear(screen_size_in_pixels, [0.0, 0.0, 0.0, 0.0]);
        painter.paint_and_update_textures(
            screen_size_in_pixels,
            pixels_per_point,
            &clipped_primitives,
            &mut self.pending_deltas.borrow_mut(),
        );
        complete_immediate_screenshots(&mut viewport.actions_requested, || {
            Arc::new(painter.read_screen_rgba(screen_size_in_pixels))
        });
        viewport
            .actions_requested
            .retain(|action| !matches!(action, ActionRequested::PaintWhileHidden));
        profiling::scope!("swap_buffers");
        if let Err(err) = gl_surface.swap_buffers(current_gl_context) {
            log::error!("swap_buffers failed: {err}");
        }
    }

    fn apply_resources(&mut self, ids: ViewportIdPair, _ctx: &egui::Context) {
        if !self.pending_deltas.borrow().is_empty() {
            let has_context = self
                .glutin
                .borrow_mut()
                .make_resource_context_current(ids.this);
            finish_glow_texture_delta(
                has_context,
                &mut *self.painter.borrow_mut(),
                &mut self.pending_deltas.borrow_mut(),
            );
        }
    }

    fn finish_viewports(
        &mut self,
        _ids: ViewportIdPair,
        ctx: &egui::Context,
        viewport_output: OrderedViewportIdMap<ViewportOutput>,
    ) {
        event_loop_context::with_current_event_loop(|event_loop| {
            self.glutin
                .borrow_mut()
                .handle_viewport_output(event_loop, ctx, &viewport_output);
        });
    }

    fn finished(&mut self, ids: ViewportIdPair, ctx: &egui::Context) {
        // Command processing can create the first available shared-context
        // surface. Finish retained work there without repainting that surface.
        self.apply_resources(ids, ctx);
    }
}

fn complete_immediate_screenshots(
    actions: &mut Vec<ActionRequested>,
    read_screen: impl FnOnce() -> Arc<egui::ColorImage>,
) {
    let mut callbacks = Vec::new();
    actions.retain(|action| {
        if let ActionRequested::Screenshot(callback) = action {
            callbacks.push(callback.clone());
            false
        } else {
            true
        }
    });
    if !callbacks.is_empty() {
        let image = read_screen();
        for callback in callbacks {
            callback.complete(Arc::clone(&image));
        }
    }
}

/// The OpenGL contexts to ask for, in order of preference.
///
/// 1. glutin's default, which is OpenGL 3.3 core.
/// 2. OpenGL ES 2.0, for drivers that have no desktop OpenGL 3.3.
/// 3. A compatibility profile without a minimum version. Drivers answer it with the newest
///    version they support (e.g. OpenGL 3.1 on Intel HD Graphics 3000 under Windows), which
///    `egui_glow` can use. This saves drivers that reject 3.3 core and cannot make ES contexts
///    through WGL either.
fn context_attributes_to_try(
    raw_window_handle: Option<raw_window_handle::RawWindowHandle>,
) -> [glutin::context::ContextAttributes; 3] {
    use glutin::context::{ContextApi, ContextAttributesBuilder, GlProfile};

    [
        ContextAttributesBuilder::new().build(raw_window_handle),
        ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(None))
            .build(raw_window_handle),
        ContextAttributesBuilder::new()
            .with_profile(GlProfile::Compatibility)
            .build(raw_window_handle),
    ]
}

/// Call `create` with `first`, then with each of `fallbacks` until one succeeds.
///
/// Logs every failure, and returns the last error if none succeeds.
fn create_first_context<A: core::fmt::Debug, T, E: core::fmt::Display>(
    first: A,
    fallbacks: impl IntoIterator<Item = A>,
    mut create: impl FnMut(&A) -> core::result::Result<T, E>,
) -> core::result::Result<T, E> {
    let mut result = create(&first);
    let mut attributes = first;
    for fallback in fallbacks {
        let Err(err) = &result else {
            break;
        };
        log::warn!(
            "Failed to create context using context attributes {attributes:?} due to error: {err}"
        );
        log::debug!("Retrying with fallback context attributes: {fallback:?}");
        result = create(&fallback);
        attributes = fallback;
    }
    result
}

#[cfg(feature = "__screenshot")]
fn save_screenshot_and_exit(
    path: &str,
    painter: &egui_glow::Painter,
    screen_size_in_pixels: [u32; 2],
) {
    assert!(
        egui::load::has_extension(path, "png"),
        "Expected EFRAME_SCREENSHOT_TO to end with '.png', got {path:?}"
    );
    let screenshot = painter.read_screen_rgba(screen_size_in_pixels);
    image::save_buffer(
        path,
        screenshot.as_raw(),
        screenshot.width() as u32,
        screenshot.height() as u32,
        image::ColorType::Rgba8,
    )
    .unwrap_or_else(|err| {
        panic!("Failed to save screenshot to {path:?}: {err}");
    });
    log::info!("Screenshot saved to {path:?}.");

    #[expect(clippy::exit)]
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::{context_attributes_to_try, create_first_context};

    #[test]
    fn context_attributes_end_with_compatibility_profile() {
        let attempts = context_attributes_to_try(None).map(|attributes| format!("{attributes:?}"));
        assert!(attempts[0].contains("profile: None") && attempts[0].contains("api: None"));
        assert!(attempts[1].contains("api: Some(Gles(None))"));
        assert!(
            attempts[2].contains("profile: Some(Compatibility)")
                && attempts[2].contains("api: None"),
            "the last resort asks for a compatibility profile of any version: {}",
            attempts[2]
        );
    }

    #[test]
    fn create_first_context_stops_at_first_success() {
        let mut tried = vec![];
        let result: Result<&str, String> = create_first_context("core", ["es", "compat"], |&a| {
            tried.push(a);
            if a == "es" {
                Ok(a)
            } else {
                Err(format!("{a} failed"))
            }
        });
        assert_eq!(result, Ok("es"));
        assert_eq!(tried, ["core", "es"]);
    }

    #[test]
    fn create_first_context_tries_every_fallback_and_returns_last_error() {
        let mut tried = vec![];
        let result: Result<(), String> = create_first_context("core", ["es", "compat"], |&a| {
            tried.push(a);
            Err(format!("{a} failed"))
        });
        assert_eq!(result, Err("compat failed".to_owned()));
        assert_eq!(tried, ["core", "es", "compat"]);

        let mut tried = vec![];
        let result: Result<&str, String> = create_first_context("core", ["es", "compat"], |&a| {
            tried.push(a);
            if a == "compat" {
                Ok(a)
            } else {
                Err(format!("{a} failed"))
            }
        });
        assert_eq!(result, Ok("compat"));
        assert_eq!(tried, ["core", "es", "compat"]);
    }
}

#[cfg(test)]
mod minimized_full_tests {
    use super::*;

    #[derive(Clone)]
    struct HeadlessDelta(TexturesDelta);

    impl std::ops::Deref for HeadlessDelta {
        type Target = TexturesDelta;
        fn deref(&self) -> &TexturesDelta {
            &self.0
        }
    }

    impl std::ops::DerefMut for HeadlessDelta {
        fn deref_mut(&mut self) -> &mut TexturesDelta {
            &mut self.0
        }
    }

    impl Drop for HeadlessDelta {
        fn drop(&mut self) {
            // Explicitly end headless ownership even if a regression assertion
            // fails, so an unapplied-delta assertion does not mask that failure.
            self.0.clear();
        }
    }

    #[derive(Default)]
    struct ResourcePainter {
        calls: Vec<(&'static str, egui::TextureId, Option<[usize; 2]>)>,
    }

    impl GlowTexturePainter for ResourcePainter {
        fn set_texture(&mut self, id: egui::TextureId, delta: &egui::epaint::ImageDelta) {
            self.calls.push(("set", id, delta.pos));
        }

        fn free_texture(&mut self, id: egui::TextureId) {
            self.calls.push(("free", id, None));
        }
    }

    fn texture_delta(id: egui::TextureId) -> HeadlessDelta {
        let mut delta = TexturesDelta::default();
        delta.push(
            id,
            egui::epaint::ImageDelta::full(
                egui::ColorImage::filled([2, 2], egui::Color32::RED),
                egui::TextureOptions::LINEAR,
            ),
        );
        delta.push(
            id,
            egui::epaint::ImageDelta::partial(
                [0, 0],
                egui::ColorImage::filled([1, 1], egui::Color32::BLUE),
                egui::TextureOptions::LINEAR,
            ),
        );
        delta.free(id);
        HeadlessDelta(delta)
    }

    #[test]
    fn minimized_full_glow_resource_work_completes_once_without_surface_paint() {
        let id = egui::TextureId::Managed(42);
        let mut pending = texture_delta(id);
        let mut painter = ResourcePainter::default();
        // This production helper exposes resource operations only: neither
        // clear/paint/swap nor post_rendering/visible ACK can run through it.
        finish_glow_texture_delta(true, &mut painter, &mut pending);
        assert_eq!(
            painter.calls,
            vec![
                ("set", id, None),
                ("set", id, Some([0, 0])),
                ("free", id, None),
            ]
        );
        assert!(pending.is_empty());
        finish_glow_texture_delta(true, &mut painter, &mut pending);
        assert_eq!(painter.calls.len(), 3);
    }

    #[test]
    fn minimized_full_glow_missing_viewport_uses_surviving_resource_context() {
        let removed = ViewportId::from_hash_of("removed");
        let sibling = ViewportId::from_hash_of("sibling");
        let available = [ViewportId::ROOT, sibling];
        let selected = select_glow_resource_viewport(removed, Some(sibling), available.into_iter());
        assert_eq!(selected, Some(sibling));
        assert_eq!(
            select_glow_resource_viewport(removed, Some(removed), [sibling].into_iter()),
            Some(sibling)
        );
        assert_eq!(
            select_glow_resource_viewport(ViewportId::ROOT, Some(sibling), available.into_iter()),
            Some(ViewportId::ROOT)
        );
        let mut pending = texture_delta(egui::TextureId::Managed(43));
        let mut painter = ResourcePainter::default();
        finish_glow_texture_delta(selected.is_some(), &mut painter, &mut pending);
        assert!(pending.is_empty());
        assert_eq!(painter.calls.len(), 3);
    }

    #[test]
    fn minimized_full_glow_no_context_keeps_existing_renderer_delta_until_available() {
        let mut pending = texture_delta(egui::TextureId::Managed(44));
        let expected = pending.clone();
        let mut painter = ResourcePainter::default();
        let selected = select_glow_resource_viewport(ViewportId::ROOT, None, [].into_iter());
        finish_glow_texture_delta(selected.is_some(), &mut painter, &mut pending);
        assert!(pending.0 == expected.0);
        assert!(painter.calls.is_empty());
        // The old owner still has all work; once a context exists, apply it
        // without requiring the original producing viewport to return.
        finish_glow_texture_delta(true, &mut painter, &mut pending);
        assert!(pending.is_empty());
        assert_eq!(painter.calls.len(), 3);
        let mut expected = expected;
        expected.clear();
    }
}

#[cfg(test)]
mod viewport_input_tests {
    use super::*;

    struct NoDisplay;

    impl raw_window_handle::HasDisplayHandle for NoDisplay {
        fn display_handle(
            &self,
        ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError>
        {
            Err(raw_window_handle::HandleError::Unavailable)
        }
    }

    #[test]
    fn pending_input_follows_viewport_lifetime_across_recreation_and_class_change() {
        let child = ViewportId::from_hash_of("input-child");
        let sibling = ViewportId::from_hash_of("input-sibling");
        let mut viewports = OrderedViewportIdMap::default();
        let mut current = None;
        for (id, event) in [(child, "old-child"), (sibling, "old-sibling")] {
            let viewport = initialize_or_update_viewport(
                &mut viewports,
                &mut current,
                ViewportIdPair::from_self_and_parent(id, ViewportId::ROOT),
                ViewportClass::Deferred,
                ViewportBuilder::default(),
                Some(Arc::new(|_| {})),
            );
            viewport.pending_raw_input = egui::RawInput {
                viewport_id: id,
                events: vec![egui::Event::Text(event.into())],
                ..Default::default()
            };
        }

        let queued_terminal_events = vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::WindowFocused(false),
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            egui::Event::Ime(egui::ImeEvent::Commit(String::new())),
        ];
        let viewport = viewports.get_mut(&child).unwrap();
        let mut state = egui_winit::State::new(
            egui::Context::default(),
            child,
            &NoDisplay,
            None,
            None,
            None,
        );
        state.egui_input_mut().events = queued_terminal_events.clone();
        viewport.egui_winit = Some(state);
        // Explicit LogicOnly child skip retains the batch in the real State.
        assert!(
            epi_integration::take_viewport_input_if(false, &mut viewport.pending_raw_input, || {
                viewport
                    .egui_winit
                    .as_mut()
                    .unwrap()
                    .egui_input_mut()
                    .take()
            },)
            .is_none()
        );

        current = Some(child);
        let viewport = initialize_or_update_viewport(
            &mut viewports,
            &mut current,
            ViewportIdPair::from_self_and_parent(child, ViewportId::ROOT),
            ViewportClass::Immediate,
            // Changing active requires native window recreation, but does not
            // end the viewport's lifetime or its unconsumed input ownership.
            ViewportBuilder::default().with_active(false),
            None,
        );
        assert!(current.is_none());
        assert!(viewport.class == ViewportClass::Immediate);
        assert!(viewport.egui_winit.is_none());
        let mut expected_events = vec![egui::Event::Text("old-child".into())];
        expected_events.extend(queued_terminal_events);
        assert_eq!(viewport.pending_raw_input.viewport_id, child);
        assert_eq!(viewport.pending_raw_input.events, expected_events);
        let input = epi_integration::take_viewport_input(&mut viewport.pending_raw_input, || {
            egui::RawInput {
                viewport_id: child,
                events: vec![egui::Event::Text("new-child".into())],
                ..Default::default()
            }
        });
        assert_eq!(input.viewport_id, child);
        expected_events.push(egui::Event::Text("new-child".into()));
        assert_eq!(input.events, expected_events);
        assert!(viewport.pending_raw_input.events.is_empty());

        // A removed viewport is a different lifetime even if the ID is reused.
        viewport.pending_raw_input.events = vec![egui::Event::Text("retired-child".into())];
        viewports.remove(&child);
        let recreated = initialize_or_update_viewport(
            &mut viewports,
            &mut current,
            ViewportIdPair::from_self_and_parent(child, ViewportId::ROOT),
            ViewportClass::Deferred,
            ViewportBuilder::default(),
            Some(Arc::new(|_| {})),
        );
        assert!(recreated.pending_raw_input.events.is_empty());
        let sibling_input = &viewports[&sibling].pending_raw_input;
        assert_eq!(sibling_input.viewport_id, sibling);
        assert_eq!(
            sibling_input.events,
            vec![egui::Event::Text("old-sibling".into())]
        );
    }
}

#[cfg(test)]
mod child_close_tests {
    use super::*;

    struct NoDisplay;

    impl raw_window_handle::HasDisplayHandle for NoDisplay {
        fn display_handle(
            &self,
        ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError>
        {
            Err(raw_window_handle::HandleError::Unavailable)
        }
    }

    fn viewports(ids: &[ViewportId]) -> OrderedViewportIdMap<Viewport> {
        let mut viewports = OrderedViewportIdMap::default();
        for &id in ids {
            let vp = initialize_or_update_viewport(
                &mut viewports,
                &mut None,
                ViewportIdPair::from_self_and_parent(id, ViewportId::ROOT),
                ViewportClass::Deferred,
                ViewportBuilder::default(),
                Some(Arc::new(|_| {
                    panic!("Close must not invoke a child callback")
                })),
            );
            vp.info.minimized = Some(true);
            vp.egui_winit = Some(egui_winit::State::new(
                egui::Context::default(),
                id,
                &NoDisplay,
                None,
                None,
                None,
            ));
        }
        viewports
    }

    #[test]
    fn glow_child_close_sources_share_dispatch_selection_and_exclude_root() {
        let child = ViewportId::from_hash_of("close-child");
        let mut viewports = viewports(&[ViewportId::ROOT, child]);
        // The OS producer publishes this same authoritative info event.
        viewports.get_mut(&child).unwrap().info.events = vec![egui::ViewportEvent::Close];
        viewports.get_mut(&ViewportId::ROOT).unwrap().info.events =
            vec![egui::ViewportEvent::Close];
        assert_eq!(glow_child_close_requests(&viewports), vec![child]);
        consume_glow_child_close(&mut viewports, child);
        assert!(glow_child_close_requests(&viewports).is_empty());
        assert!(viewports[&ViewportId::ROOT].info.close_requested());

        // Before native window initialization, programmatic Close remains a
        // deferred command. It must not depend on window creation or first UI.
        let ctx = egui::Context::default();
        viewports
            .get_mut(&child)
            .unwrap()
            .process_commands(&ctx, vec![egui::ViewportCommand::Close]);
        assert_eq!(glow_child_close_requests(&viewports), vec![child]);
        consume_glow_child_close(&mut viewports, child);
        assert!(glow_child_close_requests(&viewports).is_empty());
        viewports.get_mut(&child).unwrap().info.events = vec![egui::ViewportEvent::Close];
        assert_eq!(glow_child_close_requests(&viewports), vec![child]);
    }

    #[test]
    fn glow_child_close_consumption_preserves_normal_events_and_sibling_requests() {
        let child = ViewportId::from_hash_of("cancel-child");
        let sibling = ViewportId::from_hash_of("cancel-sibling");
        let ids = [ViewportId::ROOT, child, sibling];
        let mut viewports = viewports(&ids);
        let ordinary = vec![
            egui::Event::Text("keep".into()),
            egui::Event::PointerGone,
            egui::Event::WindowFocused(false),
            egui::Event::Ime(egui::ImeEvent::Commit(String::new())),
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        for &owner in &ids {
            let vp = viewports.get_mut(&owner).unwrap();
            vp.info.events = vec![egui::ViewportEvent::Close];
            let input = egui::RawInput {
                viewport_id: owner,
                events: ordinary.clone(),
                viewports: ids
                    .iter()
                    .map(|id| {
                        (
                            *id,
                            ViewportInfo {
                                events: vec![egui::ViewportEvent::Close],
                                minimized: Some(true),
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
                ..Default::default()
            };
            vp.pending_raw_input = input.clone();
            *vp.egui_winit.as_mut().unwrap().egui_input_mut() = input;
        }

        consume_glow_child_close(&mut viewports, child);
        assert!(!viewports[&child].info.close_requested());
        assert!(viewports[&sibling].info.close_requested());
        for &owner in &ids {
            let vp = &viewports[&owner];
            for input in [
                &vp.pending_raw_input,
                vp.egui_winit.as_ref().unwrap().egui_input(),
            ] {
                assert_eq!(input.events, ordinary);
                assert_eq!(input.viewport_id, owner);
                assert!(!input.viewports[&child].close_requested());
                assert!(input.viewports[&sibling].close_requested());
                assert!(input.viewports[&ViewportId::ROOT].close_requested());
                assert_eq!(input.viewports[&child].minimized, Some(true));
            }
        }
        // Refusing one request neither retains it nor suppresses a fresh one.
        viewports.get_mut(&child).unwrap().info.events = vec![egui::ViewportEvent::Close];
        assert!(glow_child_close_requests(&viewports).contains(&child));
    }

    struct NativeResource {
        id: ViewportId,
        kind: &'static str,
        dropped: Rc<RefCell<Vec<(ViewportId, &'static str)>>>,
    }

    impl Drop for NativeResource {
        fn drop(&mut self) {
            self.dropped.borrow_mut().push((self.id, self.kind));
        }
    }

    struct ResourceOwner {
        // Real State/pending/callback ownership plus headless native handles.
        viewport: Viewport,
        _window: NativeResource,
        _surface: NativeResource,
    }

    impl std::borrow::Borrow<Viewport> for ResourceOwner {
        fn borrow(&self) -> &Viewport {
            &self.viewport
        }
    }

    impl std::borrow::BorrowMut<Viewport> for ResourceOwner {
        fn borrow_mut(&mut self) -> &mut Viewport {
            &mut self.viewport
        }
    }

    struct SharedDelta(TexturesDelta);

    impl Drop for SharedDelta {
        fn drop(&mut self) {
            self.0.clear();
        }
    }

    struct HeadlessNativeResources {
        owners: OrderedViewportIdMap<ResourceOwner>,
        from_window: HashMap<usize, ViewportId>,
        from_viewport: OrderedViewportIdMap<usize>,
        current: Option<ViewportId>,
        focused: Option<ViewportId>,
        pending_delta: SharedDelta,
    }

    impl ChildCloseBackend for Rc<RefCell<HeadlessNativeResources>> {
        fn close_requests(&self) -> Vec<ViewportId> {
            glow_child_close_requests(&self.borrow().owners)
        }

        fn consume_close(&mut self, id: ViewportId) {
            consume_glow_child_close(&mut self.borrow_mut().owners, id);
        }

        fn retire(&mut self, ctx: &egui::Context, approval: &egui::ApprovedViewportRetirement) {
            let (approval, retired) = super::super::viewport_lifecycle::native_retirement_ids(
                ctx,
                approval,
                self.borrow().owners.iter().map(|(id, owner)| {
                    (
                        *id,
                        owner.viewport.registration,
                        owner.viewport.parent_registration,
                    )
                }),
            );
            {
                let mut resources = self.borrow_mut();
                let HeadlessNativeResources {
                    owners,
                    from_window,
                    from_viewport,
                    current,
                    focused,
                    ..
                } = &mut *resources;
                retire_glow_viewport_resources(
                    owners,
                    from_window,
                    from_viewport,
                    current,
                    focused,
                    &retired,
                );
            }
            super::super::viewport_lifecycle::finish_context_retirement(ctx, &approval);
        }
    }

    #[test]
    fn glow_child_close_dispatch_cancels_then_retires_native_and_context_without_ui() {
        let child = ViewportId::from_hash_of("dispatch-child");
        let sibling = ViewportId::from_hash_of("dispatch-sibling");
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        ctx.run_ui(Default::default(), |ui| {
            for id in [child, sibling] {
                ui.ctx()
                    .show_viewport_deferred(id, ViewportBuilder::default(), |_, _| {
                        panic!("lifecycle must not run deferred UI")
                    });
            }
        })
        .drop_without_applying_deltas();
        let dropped = Rc::new(RefCell::new(Vec::new()));
        let owners: OrderedViewportIdMap<_> = viewports(&[ViewportId::ROOT, child, sibling])
            .into_iter()
            .map(|(id, mut viewport)| {
                if id != sibling {
                    viewport.info.events.push(egui::ViewportEvent::Close);
                }
                viewport.pending_raw_input.events = vec![egui::Event::Text("kept".into())];
                let resource = |kind| NativeResource {
                    id,
                    kind,
                    dropped: Rc::clone(&dropped),
                };
                (
                    id,
                    ResourceOwner {
                        viewport,
                        _window: resource("window"),
                        _surface: resource("surface"),
                    },
                )
            })
            .collect();
        let from_window: HashMap<_, _> = owners
            .keys()
            .enumerate()
            .map(|(window, id)| (window, *id))
            .collect();
        let from_viewport = from_window
            .iter()
            .map(|(window, id)| (*id, *window))
            .collect();
        let mut pending_delta = SharedDelta(TexturesDelta::default());
        pending_delta.0.free(egui::TextureId::Managed(851));
        let expected_delta = SharedDelta(pending_delta.0.clone());
        let mut backend = Rc::new(RefCell::new(HeadlessNativeResources {
            owners,
            from_window,
            from_viewport,
            current: Some(child),
            focused: Some(child),
            pending_delta,
        }));
        struct CloseOwner {
            backend: Rc<RefCell<HeadlessNativeResources>>,
            response: crate::ViewportCloseResponse,
            calls: Vec<ViewportId>,
        }
        impl App for CloseOwner {
            fn ui(&mut self, _: &mut egui::Ui, _: &mut crate::Frame) {
                panic!("Close dispatch must not call App::ui");
            }

            fn on_viewport_close_requested(
                &mut self,
                ctx: &egui::Context,
                id: ViewportId,
            ) -> crate::ViewportCloseResponse {
                // A real hook can enter its own owner and inspect Context.
                let _exclusive_access = self.backend.borrow_mut();
                ctx.input(|_| {});
                self.calls.push(id);
                self.response
            }
        }
        let mut app = CloseOwner {
            backend: Rc::clone(&backend),
            response: crate::ViewportCloseResponse::Cancel,
            calls: Vec::new(),
        };
        dispatch_child_close_requests(&ctx, &mut app, &mut backend);
        dispatch_child_close_requests(&ctx, &mut app, &mut backend);
        assert_eq!(app.calls, [child]);
        assert!(dropped.borrow().is_empty());
        assert!(
            !backend.borrow().owners[&child]
                .viewport
                .info
                .close_requested()
        );

        // The modal has ended and the child is restored through an ordinary
        // child Full, then a parent Full. Neither may rediscover the rejection.
        let input = {
            let mut resources = backend.borrow_mut();
            let vp = &mut resources.owners.get_mut(&child).unwrap().viewport;
            vp.info.minimized = Some(false);
            let mut input = epi_integration::take_viewport_input(&mut vp.pending_raw_input, || {
                vp.egui_winit.as_mut().unwrap().egui_input_mut().take()
            });
            input.viewport_id = child;
            input.viewports = resources
                .owners
                .iter()
                .map(|(id, owner)| (*id, owner.viewport.info.clone()))
                .collect();
            input
        };
        let mut normal_child_calls = 0;
        ctx.run_ui(input, |ui| {
            normal_child_calls += 1;
            assert!(!ui.input(|input| input.viewport().close_requested()));
        })
        .drop_without_applying_deltas();
        ctx.run_ui(Default::default(), |ui| {
            for id in [child, sibling] {
                ui.ctx()
                    .show_viewport_deferred(id, ViewportBuilder::default(), |_, _| {});
            }
        })
        .drop_without_applying_deltas();
        dispatch_child_close_requests(&ctx, &mut app, &mut backend);
        assert_eq!(normal_child_calls, 1);
        assert_eq!(app.calls, [child]);

        // A fresh programmatic request uses the same production selector and
        // consumer, without requiring native initialization or another UI pass.
        backend
            .borrow_mut()
            .owners
            .get_mut(&child)
            .unwrap()
            .viewport
            .process_commands(&ctx, vec![egui::ViewportCommand::Close]);
        app.response = crate::ViewportCloseResponse::Close;
        dispatch_child_close_requests(&ctx, &mut app, &mut backend);
        dispatch_child_close_requests(&ctx, &mut app, &mut backend);
        assert_eq!(app.calls, [child, child]);
        {
            let backend = backend.borrow();
            assert!(!backend.owners.contains_key(&child));
            assert_eq!(backend.owners.len(), 2);
            assert_eq!(backend.from_window.len(), 2);
            assert_eq!(backend.from_viewport.len(), 2);
            assert!(backend.current.is_none());
            assert!(backend.focused.is_none());
            assert!(
                backend.owners[&ViewportId::ROOT]
                    .viewport
                    .info
                    .close_requested()
            );
            assert_eq!(
                backend.owners[&sibling].viewport.pending_raw_input.events,
                vec![egui::Event::Text("kept".into())]
            );
            assert!(backend.pending_delta.0 == expected_delta.0);
        }
        assert_eq!(
            dropped.borrow().as_slice(),
            [(child, "window"), (child, "surface")]
        );
        // The adapter used the actual non-drawing Context retirement, not just
        // native bookkeeping. Later sibling/root Full outputs cannot recreate it.
        assert!(ctx.retire_viewport(child).is_empty());
        for id in [sibling, ViewportId::ROOT] {
            let output = ctx.run_ui(
                egui::RawInput {
                    viewport_id: id,
                    viewports: backend
                        .borrow()
                        .owners
                        .iter()
                        .map(|(id, owner)| (*id, owner.viewport.info.clone()))
                        .collect(),
                    ..Default::default()
                },
                |_| {},
            );
            assert!(!output.viewport_output.contains_key(&child));
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn glow_child_close_retirement_drops_native_owners_and_subtree_using_normal_gc_cleanup() {
        let child = ViewportId::from_hash_of("retire-child");
        let descendant = ViewportId::from_hash_of("retire-descendant");
        let sibling = ViewportId::from_hash_of("retire-sibling");
        let dropped = Rc::new(RefCell::new(Vec::new()));
        let mut viewports = viewports(&[ViewportId::ROOT, child, descendant, sibling]);
        viewports.get_mut(&descendant).unwrap().ids.parent = child;
        viewports
            .get_mut(&sibling)
            .unwrap()
            .pending_raw_input
            .events = vec![egui::Event::Text("sibling pending".into())];
        viewports
            .get_mut(&sibling)
            .unwrap()
            .egui_winit
            .as_mut()
            .unwrap()
            .egui_input_mut()
            .events = vec![egui::Event::Text("sibling State".into())];
        let retired =
            glow_viewport_subtree(child, viewports.iter().map(|(id, vp)| (*id, vp.ids.parent)));
        assert_eq!(retired, egui::ViewportIdSet::from_iter([child, descendant]));

        let mut owners: OrderedViewportIdMap<_> = viewports
            .into_iter()
            .map(|(id, viewport)| {
                let resource = |kind| NativeResource {
                    id,
                    kind,
                    dropped: Rc::clone(&dropped),
                };
                (
                    id,
                    ResourceOwner {
                        viewport,
                        _window: resource("window"),
                        _surface: resource("surface"),
                    },
                )
            })
            .collect();
        let mut from_window: HashMap<_, _> = owners
            .keys()
            .enumerate()
            .map(|(index, id)| (index, *id))
            .collect();
        let mut from_viewport: OrderedViewportIdMap<_> = from_window
            .iter()
            .map(|(window, id)| (*id, *window))
            .collect();
        let sibling_window = from_viewport[&sibling];
        let mut current = Some(descendant);
        let mut focused = Some(child);
        let mut pending_delta = SharedDelta(TexturesDelta::default());
        pending_delta.0.free(egui::TextureId::Managed(850));
        let expected_delta = SharedDelta(pending_delta.0.clone());

        retire_glow_viewport_resources(
            &mut owners,
            &mut from_window,
            &mut from_viewport,
            &mut current,
            &mut focused,
            &retired,
        );
        assert_eq!(owners.len(), 2);
        assert!(owners.contains_key(&ViewportId::ROOT));
        assert!(owners.contains_key(&sibling));
        assert!(!owners.contains_key(&child));
        assert!(!owners.contains_key(&descendant));
        assert_eq!(from_window.len(), 2);
        assert_eq!(from_viewport.len(), 2);
        assert_eq!(from_window[&sibling_window], sibling);
        assert!(current.is_none());
        assert!(focused.is_none());
        assert_eq!(dropped.borrow().len(), 4);
        for &id in &[child, descendant] {
            for kind in ["window", "surface"] {
                assert!(dropped.borrow().contains(&(id, kind)));
            }
        }
        assert_eq!(
            owners[&sibling]
                .viewport
                .egui_winit
                .as_ref()
                .unwrap()
                .egui_input()
                .events,
            vec![egui::Event::Text("sibling State".into())]
        );
        assert_eq!(
            owners[&sibling].viewport.pending_raw_input.events,
            vec![egui::Event::Text("sibling pending".into())]
        );
        assert!(pending_delta.0 == expected_delta.0);

        // Reusing the same GC helper is idempotent; a surviving resource
        // context can apply the original shared delta later.
        current = Some(sibling);
        focused = Some(sibling);
        retire_glow_viewport_resources(
            &mut owners,
            &mut from_window,
            &mut from_viewport,
            &mut current,
            &mut focused,
            &retired,
        );
        assert_eq!(current, Some(sibling));
        assert_eq!(focused, Some(sibling));
        assert_eq!(dropped.borrow().len(), 4);
    }
}

#[cfg(test)]
mod immediate_screenshot_tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn minimized_full_immediate_real_entry_preserves_child_output_and_paint_ownership() {
        super::super::frame_output::immediate_tests::check_immediate_entry(|backend, viewport| {
            render_immediate_viewport(backend, viewport);
        });
    }

    #[test]
    fn queued_child_screenshots_complete_once_and_preserve_other_actions() {
        let images = Arc::new(Mutex::new(Vec::new()));
        let received = Arc::clone(&images);
        let callback =
            egui::ScreenshotCallback::new(move |image| received.lock().unwrap().push(image));
        let mut actions = vec![
            ActionRequested::Copy,
            ActionRequested::Screenshot(callback.clone()),
            ActionRequested::PaintWhileHidden,
            ActionRequested::Screenshot(callback.clone()),
            ActionRequested::Paste,
        ];
        let image = Arc::new(egui::ColorImage::filled([2, 1], egui::Color32::RED));
        let mut reads = 0;
        complete_immediate_screenshots(&mut actions, || {
            reads += 1;
            Arc::clone(&image)
        });
        assert_eq!(reads, 1);
        assert!(matches!(
            actions.as_slice(),
            [
                ActionRequested::Copy,
                ActionRequested::PaintWhileHidden,
                ActionRequested::Paste
            ]
        ));
        assert_eq!(images.lock().unwrap().len(), 1);
        assert!(Arc::ptr_eq(&images.lock().unwrap()[0], &image));
        complete_immediate_screenshots(&mut actions, || panic!("no capture remains"));
        callback.complete(Arc::clone(&image));
        assert_eq!(images.lock().unwrap().len(), 1);
    }
}

#[cfg(test)]
mod viewport_retirement_tests {
    use super::super::viewport_lifecycle::{
        self as lifecycle,
        viewport_retirement_tests::{self as scenarios, Adapter},
    };
    use super::*;

    struct Headless {
        viewports: OrderedViewportIdMap<Viewport>,
        from_window: HashMap<usize, ViewportId>,
        from_viewport: OrderedViewportIdMap<usize>,
        current: Option<ViewportId>,
        focused: Option<ViewportId>,
        delta: TexturesDelta,
    }
    impl Drop for Headless {
        fn drop(&mut self) {
            self.delta.clear();
        }
    }
    impl Headless {
        fn remove(&mut self, retired: &egui::ViewportIdSet) {
            // Same child-first map/surface/input ownership removal as native
            // GlutinWindowContext, with fake WindowKeys and no GL/desktop use.
            let order = lifecycle::child_first_ids(
                retired,
                self.viewports.iter().map(|(id, vp)| (*id, vp.ids.parent)),
            );
            let removed: Vec<_> = order
                .into_iter()
                .filter_map(|id| self.viewports.remove(&id))
                .collect();
            retire_glow_viewport_resources(
                &mut self.viewports,
                &mut self.from_window,
                &mut self.from_viewport,
                &mut self.current,
                &mut self.focused,
                retired,
            );
            drop(removed);
        }
    }
    impl ChildCloseBackend for Headless {
        fn close_requests(&self) -> Vec<ViewportId> {
            glow_child_close_requests(&self.viewports)
        }
        fn consume_close(&mut self, id: ViewportId) {
            consume_glow_child_close(&mut self.viewports, id);
        }
        fn registration(
            &self,
            _: &egui::Context,
            id: ViewportId,
        ) -> Option<egui::ViewportRegistration> {
            self.viewports.get(&id).and_then(|vp| vp.registration)
        }
        fn retire(&mut self, ctx: &egui::Context, approval: &egui::ApprovedViewportRetirement) {
            let (approval, retired) = lifecycle::native_retirement_ids(
                ctx,
                approval,
                self.viewports
                    .iter()
                    .map(|(id, vp)| (*id, vp.registration, vp.parent_registration)),
            );
            self.remove(&retired);
            lifecycle::finish_context_retirement(ctx, &approval);
        }
        fn gc(&mut self, ctx: &egui::Context) {
            let retired: egui::ViewportIdSet = self
                .viewports
                .iter()
                .filter_map(|(id, vp)| {
                    (*id != ViewportId::ROOT && ctx.viewport_registration(*id) != vp.registration)
                        .then_some(*id)
                })
                .collect();
            self.remove(&retired);
            let ids: Vec<_> = retired
                .into_iter()
                .filter(|id| ctx.viewport_registration(*id).is_none())
                .collect();
            egui_ime::retire_viewport_ime_policy(ctx, &ids);
        }
        fn collect_retirement_delta(&mut self, ctx: &egui::Context) {
            lifecycle::append_texture_delta(
                &mut self.delta,
                ctx.take_viewport_retirement_texture_delta(),
            );
        }
    }
    impl Adapter for Headless {
        fn new(_: &egui::Context) -> Self {
            Self {
                viewports: Default::default(),
                from_window: Default::default(),
                from_viewport: Default::default(),
                current: None,
                focused: None,
                delta: Default::default(),
            }
        }
        fn sync(&mut self, ctx: &egui::Context, outputs: &OrderedViewportIdMap<ViewportOutput>) {
            for (&id, output) in outputs {
                if ctx.viewport_registration(id) != Some(output.registration) {
                    continue;
                }
                let viewport = initialize_or_update_viewport(
                    &mut self.viewports,
                    &mut self.current,
                    ViewportIdPair::from_self_and_parent(id, output.parent),
                    output.class,
                    output.builder.clone(),
                    output.viewport_ui_cb.clone(),
                );
                viewport.registration = Some(output.registration);
                viewport.parent_registration = output.parent_registration;
                viewport.deferred_commands.extend(output.commands.clone());
                viewport.pending_raw_input.viewport_id = id;
                if id != ViewportId::ROOT {
                    viewport
                        .pending_raw_input
                        .viewports
                        .entry(id)
                        .or_default()
                        .parent = Some(viewport.ids.parent);
                }
                if viewport.pending_raw_input.events.is_empty() {
                    viewport
                        .pending_raw_input
                        .events
                        .push(egui::Event::Text(format!("pending-{id:?}")));
                }
            }
        }
        fn registration_for(&self, id: ViewportId) -> Option<egui::ViewportRegistration> {
            self.viewports.get(&id).and_then(|vp| vp.registration)
        }
        fn set_close(&mut self, id: ViewportId) {
            self.viewports
                .get_mut(&id)
                .unwrap()
                .info
                .events
                .push(egui::ViewportEvent::Close);
        }
        fn pending(&self, id: ViewportId) -> Vec<egui::Event> {
            self.viewports[&id].pending_raw_input.events.clone()
        }
        fn accept_delta(&mut self, delta: egui::TexturesDelta) {
            lifecycle::append_texture_delta(&mut self.delta, delta);
        }
        fn take_delta(&mut self) -> egui::TexturesDelta {
            core::mem::take(&mut self.delta)
        }
        fn replace_stale_native(&mut self, ctx: &egui::Context, id: ViewportId) {
            let stale = lifecycle::stale_native_subtree(
                ctx,
                id,
                self.viewports
                    .iter()
                    .map(|(id, vp)| (*id, vp.registration, vp.parent_registration)),
            );
            if !stale.is_empty() {
                self.remove(&stale);
                lifecycle::append_texture_delta(
                    &mut self.delta,
                    ctx.tex_manager().write().take_delta(),
                );
            }
        }
        fn seed_stale_native(&mut self, ctx: &egui::Context, id: ViewportId) -> egui::TextureId {
            let texture = scenarios::stale_texture(ctx);
            let texture_id = texture.id();
            let viewport = self.viewports.get_mut(&id).unwrap();
            viewport
                .pending_raw_input
                .events
                .push(scenarios::stale_event());
            viewport
                .deferred_commands
                .push(egui::ViewportCommand::Close);
            let mut input = egui_winit::State::new(
                ctx.clone(),
                id,
                &scenarios::HeadlessDisplay,
                None,
                None,
                None,
            );
            *input.egui_input_mut() = scenarios::input(id, false);
            input.egui_input_mut().events.push(scenarios::stale_event());
            viewport.egui_winit = Some(input);
            viewport.viewport_ui_cb = Some(Arc::new(move |_| {
                let _ = &texture;
            }));
            texture_id
        }
        fn native_input(&self, id: ViewportId) -> Vec<egui::Event> {
            let viewport = &self.viewports[&id];
            let mut events = viewport.pending_raw_input.events.clone();
            if let Some(input) = &viewport.egui_winit {
                events.extend(input.egui_input().events.clone());
            }
            events
        }
    }
    #[test]
    fn viewport_retirement_glow_full_logic_gc_discard() {
        scenarios::full_logic_gc_discard::<Headless>();
    }
    #[test]
    fn viewport_retirement_glow_logic_only() {
        scenarios::logic_only_and_pass_outside_hook::<Headless>();
    }
    #[test]
    fn viewport_retirement_glow_close_request_separation() {
        scenarios::close_request_separation::<Headless>();
    }
    #[test]
    fn viewport_retirement_glow_context_gc_native_descendants_ime() {
        scenarios::context_gc_native_descendants_and_ime::<Headless>();
    }
    #[test]
    fn viewport_retirement_glow_old_output_reopen_duplicate() {
        scenarios::old_output_reopen_and_duplicate::<Headless>();
    }
    #[test]
    fn viewport_retirement_glow_overlapping_gc_root_close() {
        scenarios::overlapping_gc_and_root_close::<Headless>();
    }
    #[test]
    fn viewport_retirement_glow_delta_callback_drop() {
        scenarios::delta_and_callback_drop::<Headless>();
    }
    #[test]
    fn viewport_retirement_glow_immediate_nested() {
        scenarios::immediate_nested::<Headless>();
    }
    #[test]
    fn viewport_retirement_glow_hook_captures_lifetime_before_reopen() {
        scenarios::legacy_hook_captures_lifetime_before_reopen::<Headless>();
    }
    #[test]
    fn viewport_retirement_glow_discard_immediate_reopen_replaces_native() {
        scenarios::discard_immediate_reopen_replaces_native::<Headless>();
    }
    #[test]
    fn discard_texture_delivery_glow_native_callback_drop_gc_reopen_ime() {
        scenarios::discard_texture_delivery_native_reopen::<Headless>();
    }

    #[test]
    fn discard_texture_delivery_glow_resource_consumer_pending_unavailable_order() {
        // Fake GL calls through the production resource consumer: this proves
        // adapter order only. No real GL context/window was created.
        #[derive(Default)]
        struct Painter(Vec<(egui::TextureId, Option<[usize; 2]>, bool)>);
        impl GlowTexturePainter for Painter {
            fn set_texture(&mut self, id: egui::TextureId, delta: &egui::epaint::ImageDelta) {
                self.0.push((id, delta.pos, false));
            }
            fn free_texture(&mut self, id: egui::TextureId) {
                self.0.push((id, None, true));
            }
        }
        let ctx = egui::Context::default();
        let mut handle = Some(scenarios::stale_texture(&ctx));
        let id = handle.as_ref().unwrap().id();
        let pending = Rc::new(RefCell::new(TexturesDelta::default()));
        let sink = pending.clone();
        let _scope = ctx.enter_discard_texture_delivery(move |delta| {
            lifecycle::append_texture_delta(&mut sink.borrow_mut(), delta);
        });
        let mut pass = 0;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            if pass == 0 {
                ui.ctx().request_discard("GL unavailable");
            } else {
                drop(handle.take());
            }
            pass += 1;
        });
        lifecycle::append_texture_delta(
            &mut pending.borrow_mut(),
            core::mem::take(&mut output.textures_delta),
        );
        output.drop_without_applying_deltas();
        let mut painter = Painter::default();
        finish_glow_texture_delta(false, &mut painter, &mut pending.borrow_mut());
        assert!(painter.0.is_empty());
        assert_eq!(pending.borrow().set[&id].len(), 2);
        assert!(pending.borrow().free.contains(&id));
        finish_glow_texture_delta(true, &mut painter, &mut pending.borrow_mut());
        let calls: Vec<_> = painter.0.into_iter().filter(|call| call.0 == id).collect();
        assert_eq!(
            calls,
            [
                (id, None, false),
                (id, Some([1, 0]), false),
                (id, None, true)
            ]
        );
        assert!(pending.borrow().is_empty());
    }

    #[test]
    fn retirement_texture_delivery_glow_skips_empty_context_and_delivers_free_only() {
        // Fake GL calls through the real consumer and lazy retirement dispatcher.
        // No GL context or native window is created by this regression.
        #[derive(Default)]
        struct Painter(Vec<(egui::TextureId, bool)>);
        impl GlowTexturePainter for Painter {
            fn set_texture(&mut self, id: egui::TextureId, delta: &egui::epaint::ImageDelta) {
                assert!(delta.image.width() > 0 && delta.image.height() > 0);
                self.0.push((id, false));
            }
            fn free_texture(&mut self, id: egui::TextureId) {
                self.0.push((id, true));
            }
        }
        let ctx = egui::Context::default();
        let mut painter = Painter::default();
        let mut pending = TexturesDelta::default();
        let mut context_entries = 0;
        let deliver = |pending: &mut TexturesDelta, painter: &mut Painter, entries: &mut usize| {
            lifecycle::finish_retirement_texture_delta(&ctx, pending, |delta| {
                *entries += 1; // Represents make_resource_context_current, before GL calls.
                finish_glow_texture_delta(true, painter, delta);
            });
        };
        deliver(&mut pending, &mut painter, &mut context_entries);
        assert_eq!(
            context_entries, 0,
            "pre-Full reservation must stay with Context"
        );
        assert!(painter.0.is_empty());
        let mut output = ctx.run_ui(Default::default(), |_| {});
        lifecycle::append_texture_delta(&mut pending, core::mem::take(&mut output.textures_delta));
        output.drop_without_applying_deltas();
        deliver(&mut pending, &mut painter, &mut context_entries);
        assert_eq!(painter.0, [(egui::TextureId::default(), false)]);
        painter.0.clear();
        context_entries = 0;
        for _ in 0..32 {
            deliver(&mut pending, &mut painter, &mut context_entries);
        }
        assert_eq!(context_entries, 0, "idle events must not switch GL context");
        assert!(painter.0.is_empty());

        let texture = ctx.load_texture(
            "retirement-delivery",
            egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
            egui::TextureOptions::NEAREST,
        );
        let id = texture.id();
        deliver(&mut pending, &mut painter, &mut context_entries);
        assert_eq!(painter.0, [(id, false)]);
        drop(texture);
        deliver(&mut pending, &mut painter, &mut context_entries);
        let free_batch_consumed = pending.is_empty();
        // Headless negative-control cleanup avoids masking the primary assertion
        // with a second panic from an unapplied TexturesDelta's Drop.
        pending.clear();
        assert_eq!(painter.0, [(id, false), (id, true)]);
        assert!(free_batch_consumed, "free-only batch must be consumed");
        assert_eq!(context_entries, 2, "free-only must enter the consumer");
        deliver(&mut pending, &mut painter, &mut context_entries);
        assert_eq!(context_entries, 2);
        assert!(pending.is_empty());
    }
}
