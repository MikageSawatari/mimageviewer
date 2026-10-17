//! Note that this file contains code very similar to [`super::glow_integration`].
//! When making changes to one you often also want to apply it to the other.
//!
//! This is also very complex code, and not very pretty.
//! There is a bunch of improvements we could do,
//! like removing a bunch of `unwraps`.

use core::{cell::RefCell, num::NonZeroU32};
use std::{rc::Rc, sync::Arc, time::Instant};

use egui_winit::ActionRequested;
use parking_lot::Mutex;
use raw_window_handle::{HasDisplayHandle as _, HasWindowHandle as _};
use winit::{
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    window::{Window, WindowId},
};

use ahash::HashMap;
use egui::{
    DeferredViewportUiCallback, FullOutput, ImmediateViewport, OrderedViewportIdMap, TexturesDelta,
    ViewportBuilder, ViewportClass, ViewportId, ViewportIdPair, ViewportIdSet, ViewportInfo,
    ViewportOutput,
};
#[cfg(feature = "accesskit")]
use egui_winit::accesskit_winit;
use winit_integration::UserEvent;

use crate::{
    App, AppCreator, CreationContext, NativeOptions, Result, Storage,
    native::{
        epi_integration::EpiIntegration,
        winit_integration::{
            EventResult, PassMode, RenderTarget, RepaintNowReason, sleep_if_invisible_or_minimized,
        },
    },
};

use super::{epi_integration, event_loop_context, winit_integration, winit_integration::WinitApp};

#[cfg(all(test, target_os = "windows"))]
pub(super) mod render_phase_test_gate {
    use std::{
        io::{Read as _, Write as _},
        net::TcpStream,
        sync::{Mutex, OnceLock},
    };

    enum GateState {
        Disabled,
        Bootstrap,
        Ready(TcpStream),
        Armed(TcpStream),
        Released,
    }

    static STATE: OnceLock<Mutex<GateState>> = OnceLock::new();

    fn state() -> &'static Mutex<GateState> {
        STATE.get_or_init(|| {
            let enabled = std::env::var("EFRAME_RENDER_PHASE_TEST_CHILD").as_deref() == Ok("1");
            Mutex::new(if enabled {
                GateState::Bootstrap
            } else {
                GateState::Disabled
            })
        })
    }

    #[allow(unsafe_code)]
    fn performance_counter() -> i64 {
        let mut counter = 0;
        let success = unsafe {
            windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut counter)
        };
        assert_ne!(success, 0, "QueryPerformanceCounter failed");
        counter
    }

    pub(super) fn before_surface_acquire() {
        let mut state = state().lock().expect("render phase test gate poisoned");
        let GateState::Armed(stream) = &mut *state else {
            return;
        };

        // The caller records SendMessageTimeoutW's return before releasing this barrier.
        // With the old inline WM_PAINT path both sides wait here until the 500 ms send
        // timeout, so a successful send is itself part of the proof.
        let ack_port = std::env::var("EFRAME_RENDER_PHASE_TEST_ACK_PORT")
            .expect("test sender acknowledgement port must be provided");
        let mut ack_stream = TcpStream::connect(("127.0.0.1", ack_port.parse::<u16>().unwrap()))
            .expect("connect sender-return acknowledgement");
        let mut sender_returned = [0_u8; 1];
        ack_stream
            .read_exact(&mut sender_returned)
            .expect("sender must return before outer gate entry");

        let entered_at = performance_counter();
        writeln!(stream, "GATE_ENTER {entered_at}")
            .expect("report outer paint gate entry to parent");
        stream.flush().expect("flush outer paint gate entry");
        let mut release = [0_u8; 1];
        stream
            .read_exact(&mut release)
            .expect("parent must release the painter gate");
        *state = GateState::Released;
    }

    pub(crate) fn arm() {
        let mut state = state().lock().expect("render phase test gate poisoned");
        let current = std::mem::replace(&mut *state, GateState::Disabled);
        *state = match current {
            GateState::Ready(stream) => GateState::Armed(stream),
            other => other,
        };
    }

    pub(super) fn after_paint() {
        let mut state = state().lock().expect("render phase test gate poisoned");
        if !matches!(*state, GateState::Bootstrap) {
            return;
        }

        let port = std::env::var("EFRAME_RENDER_PHASE_TEST_GATE_PORT")
            .expect("test gate port must be provided");
        let mut stream = TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap()))
            .expect("connect render phase test parent");
        writeln!(stream, "READY").expect("report completed bootstrap paint");
        stream.flush().expect("flush bootstrap-ready marker");
        *state = GateState::Ready(stream);
    }
}

// ----------------------------------------------------------------------------
// Types:

pub struct WgpuWinitApp<'app> {
    repaint_proxy: Arc<Mutex<EventLoopProxy<UserEvent>>>,
    app_name: String,
    native_options: NativeOptions,

    /// Set at initialization, then taken and set to `None` in `init_run_state`.
    app_creator: Option<AppCreator<'app>>,

    /// Set when we are actually up and running.
    running: Option<WgpuWinitRunning<'app>>,

    /// An optional pre-existing egui context. If `Some`, it is used instead of
    /// creating a new one via [`winit_integration::create_egui_context`]. Taken during initialization.
    egui_ctx: Option<egui::Context>,
}

/// State that is initialized when the application is first starts running via
/// a Resumed event. On Android this ensures that any graphics state is only
/// initialized once the application has an associated `SurfaceView`.
struct WgpuWinitRunning<'app> {
    integration: EpiIntegration,

    /// The users application.
    app: Box<dyn 'app + App>,

    /// Wrapped in an `Rc<RefCell<…>>` so it can be re-entrantly shared via a weak-pointer.
    shared: Rc<RefCell<SharedState>>,
}

/// Everything needed by the immediate viewport renderer.\
///
/// This is shared by all viewports.
///
/// Wrapped in an `Rc<RefCell<…>>` so it can be re-entrantly shared via a weak-pointer.
pub struct SharedState {
    egui_ctx: egui::Context,
    viewports: Viewports,
    painter: egui_wgpu::winit::Painter,

    /// Shared renderer delivery owner, including deltas from disappearing viewports.
    pending_deltas: TexturesDelta,
    viewport_from_window: HashMap<WindowId, ViewportId>,
    focused_viewport: Option<ViewportId>,
    resized_viewport: Option<ViewportId>,

    /// Passed on to each [`egui_winit::State`]. See [`NativeOptions::clipboard_shortcuts`].
    clipboard_shortcuts: bool,
}

pub type Viewports = egui::OrderedViewportIdMap<Viewport>;

pub struct Viewport {
    ids: ViewportIdPair,
    registration: Option<egui::ViewportRegistration>,
    parent_registration: Option<egui::ViewportRegistration>,
    class: ViewportClass,
    builder: ViewportBuilder,
    deferred_commands: Vec<egui::viewport::ViewportCommand>,
    info: ViewportInfo,
    actions_requested: Vec<ActionRequested>,

    /// `None` for sync viewports.
    viewport_ui_cb: Option<Arc<DeferredViewportUiCallback>>,

    /// Window surface state that's initialized when the app starts running via a Resumed event
    /// and on Android will also be destroyed if the application is paused.
    window: Option<Arc<Window>>,

    /// Monotonic identity of the current window surface/configuration.
    surface_generation: u64,

    /// `window` and `egui_winit` are initialized together.
    egui_winit: Option<egui_winit::State>,

    /// Input not yet consumed by this logical viewport's UI pass.
    pending_raw_input: egui::RawInput,
}

impl Drop for SharedState {
    fn drop(&mut self) {
        // The renderer/Context delivery destination ends here, not when a viewport closes.
        self.pending_deltas.clear();
    }
}

// ----------------------------------------------------------------------------

impl<'app> WgpuWinitApp<'app> {
    pub fn new(
        event_loop: &EventLoop<UserEvent>,
        app_name: &str,
        native_options: NativeOptions,
        egui_ctx: Option<egui::Context>,
        app_creator: AppCreator<'app>,
    ) -> Self {
        profiling::function_scope!();

        #[cfg(feature = "__screenshot")]
        assert!(
            std::env::var("EFRAME_SCREENSHOT_TO").is_err(),
            "EFRAME_SCREENSHOT_TO not yet implemented for wgpu backend"
        );

        Self {
            repaint_proxy: Arc::new(Mutex::new(event_loop.create_proxy())),
            app_name: app_name.to_owned(),
            native_options,
            running: None,
            app_creator: Some(app_creator),
            egui_ctx,
        }
    }

    /// Create a window for all viewports lacking one.
    fn initialized_all_windows(&mut self, event_loop: &ActiveEventLoop) {
        let Some(running) = &mut self.running else {
            return;
        };
        let mut shared = running.shared.borrow_mut();
        let SharedState {
            viewports,
            painter,
            viewport_from_window,
            clipboard_shortcuts,
            ..
        } = &mut *shared;

        for viewport in viewports.values_mut() {
            viewport.initialize_window(
                event_loop,
                &running.integration.egui_ctx,
                viewport_from_window,
                painter,
                *clipboard_shortcuts,
            );
        }
    }

    #[cfg(target_os = "android")]
    fn recreate_window(&self, event_loop: &ActiveEventLoop, running: &WgpuWinitRunning<'app>) {
        let SharedState {
            egui_ctx,
            viewports,
            viewport_from_window,
            painter,
            clipboard_shortcuts,
            ..
        } = &mut *running.shared.borrow_mut();

        initialize_or_update_viewport(
            viewports,
            ViewportIdPair::ROOT,
            ViewportClass::Root,
            self.native_options.viewport.clone(),
            None,
            painter,
        )
        .initialize_window(
            event_loop,
            egui_ctx,
            viewport_from_window,
            painter,
            *clipboard_shortcuts,
        );
    }

    #[cfg(target_os = "android")]
    fn drop_window(&mut self) -> Result<(), egui_wgpu::WgpuError> {
        if let Some(running) = &mut self.running {
            let mut shared = running.shared.borrow_mut();
            shared.viewports.remove(&ViewportId::ROOT);
            pollster::block_on(shared.painter.set_window(ViewportId::ROOT, None))?;
        }
        Ok(())
    }

    fn init_run_state(
        &mut self,
        egui_ctx: egui::Context,
        event_loop: &ActiveEventLoop,
        storage: Option<Box<dyn Storage>>,
        window: Window,
        builder: ViewportBuilder,
    ) -> crate::Result<&mut WgpuWinitRunning<'app>> {
        profiling::function_scope!();
        // Inject the display handle into the wgpu setup so that wgpu can create
        // surfaces on platforms that require it (e.g. GLES on Wayland).
        let mut wgpu_options = self.native_options.wgpu_options.clone();
        if let egui_wgpu::WgpuSetup::CreateNew(ref mut create_new) = wgpu_options.wgpu_setup
            && create_new.display_handle.is_none()
        {
            create_new.display_handle = Some(Box::new(event_loop.owned_display_handle()));
        }
        let mut painter = pollster::block_on(egui_wgpu::winit::Painter::new(
            egui_ctx.clone(),
            wgpu_options,
            self.native_options.viewport.transparent.unwrap_or(false),
            egui_wgpu::RendererOptions {
                msaa_samples: self.native_options.multisampling as _,
                depth_stencil_format: egui_wgpu::depth_format_from_bits(
                    self.native_options.depth_buffer,
                    self.native_options.stencil_buffer,
                ),
                dithering: self.native_options.dithering,
                ..Default::default()
            },
        ));

        let mut viewport_info = ViewportInfo::default();
        egui_winit::update_viewport_info(&mut viewport_info, &egui_ctx, &window, true);

        {
            // Tell egui right away about native_pixels_per_point etc,
            // so that the app knows about it during app creation:
            let pixels_per_point = egui_winit::pixels_per_point(&egui_ctx, &window);

            egui_ctx.input_mut(|i| {
                i.raw
                    .viewports
                    .insert(ViewportId::ROOT, viewport_info.clone());
                i.pixels_per_point = pixels_per_point;
            });
        }

        let window = Arc::new(window);

        {
            profiling::scope!("set_window");
            pollster::block_on(painter.set_window(ViewportId::ROOT, Some(Arc::clone(&window))))?;
        }

        let wgpu_render_state = painter.render_state();

        let integration = EpiIntegration::new(
            egui_ctx.clone(),
            &window,
            &self.app_name,
            &self.native_options,
            storage,
            #[cfg(feature = "glow")]
            None,
            #[cfg(feature = "glow")]
            None,
            wgpu_render_state.clone(),
        );

        {
            let event_loop_proxy = Arc::clone(&self.repaint_proxy);

            egui_ctx.set_request_repaint_callback(move |info| {
                log::trace!("request_repaint_callback: {info:?}");
                let when = Instant::now() + info.delay;
                let cumulative_pass_nr = info.current_cumulative_pass_nr;

                event_loop_proxy
                    .lock()
                    .send_event(UserEvent::RequestRepaint {
                        when,
                        cumulative_pass_nr,
                        viewport_id: info.viewport_id,
                    })
                    .ok();
            });
        }

        let mut egui_winit = egui_winit::State::new(
            egui_ctx.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            event_loop.system_theme(),
            painter.max_texture_side(),
        );
        egui_winit.set_clipboard_shortcuts(self.native_options.clipboard_shortcuts);

        #[cfg(feature = "accesskit")]
        {
            let event_loop_proxy = self.repaint_proxy.lock().clone();
            egui_winit.init_accesskit(event_loop, &window, event_loop_proxy);
        }

        let app_creator = core::mem::take(&mut self.app_creator)
            .expect("Single-use AppCreator has unexpectedly already been taken");

        crate::maybe_attach_inspection_plugin(&egui_ctx, Some(self.app_name.clone()));

        let cc = CreationContext {
            egui_ctx: egui_ctx.clone(),
            integration_info: integration.frame.info().clone(),
            storage: integration.frame.storage(),
            #[cfg(feature = "glow")]
            gl: None,
            #[cfg(feature = "glow")]
            get_proc_address: None,
            wgpu_render_state,
            window: Some(Arc::clone(&window)),
            raw_display_handle: window.display_handle().map(|h| h.as_raw()),
            raw_window_handle: window.window_handle().map(|h| h.as_raw()),
        };
        let app = {
            profiling::scope!("user_app_creator");
            app_creator(&cc).map_err(crate::Error::AppCreation)?
        };

        let mut viewport_from_window = HashMap::default();
        viewport_from_window.insert(window.id(), ViewportId::ROOT);

        let mut viewports = Viewports::default();
        viewports.insert(
            ViewportId::ROOT,
            Viewport {
                ids: ViewportIdPair::ROOT,
                registration: egui_ctx.viewport_registration(ViewportId::ROOT),
                parent_registration: None,
                class: ViewportClass::Root,
                builder,
                deferred_commands: vec![],
                info: viewport_info,
                actions_requested: Default::default(),
                viewport_ui_cb: None,
                window: Some(window),
                surface_generation: 1,
                egui_winit: Some(egui_winit),
                pending_raw_input: Default::default(),
            },
        );

        let shared = Rc::new(RefCell::new(SharedState {
            egui_ctx,
            viewport_from_window,
            viewports,
            painter,
            pending_deltas: Default::default(),
            focused_viewport: Some(ViewportId::ROOT),
            resized_viewport: None,
            clipboard_shortcuts: self.native_options.clipboard_shortcuts,
        }));

        {
            // Create a weak pointer so that we don't keep state alive for too long.
            let shared = Rc::downgrade(&shared);
            let beginning = integration.beginning;

            egui::Context::set_immediate_viewport_renderer(move |_egui_ctx, immediate_viewport| {
                if let Some(shared) = shared.upgrade() {
                    render_immediate_viewport(
                        &mut NativeWgpuImmediateBackend::new(beginning, &shared),
                        immediate_viewport,
                    );
                } else {
                    log::warn!("render_sync_callback called after window closed");
                }
            });
        }

        Ok(self.running.insert(WgpuWinitRunning {
            integration,
            app,
            shared,
        }))
    }
}

impl WinitApp for WgpuWinitApp<'_> {
    fn egui_ctx(&self) -> Option<&egui::Context> {
        self.running.as_ref().map(|r| &r.integration.egui_ctx)
    }

    fn window(&self, window_id: WindowId) -> Option<Arc<Window>> {
        self.running
            .as_ref()
            .and_then(|r| {
                let shared = r.shared.borrow();
                let id = shared.viewport_from_window.get(&window_id)?;
                shared.viewports.get(id).map(|v| v.window.clone())
            })
            .flatten()
    }

    fn window_id_from_viewport_id(&self, id: ViewportId) -> Option<WindowId> {
        Some(
            self.running
                .as_ref()?
                .shared
                .borrow()
                .viewports
                .get(&id)?
                .window
                .as_ref()?
                .id(),
        )
    }

    fn render_target(&self, window_id: WindowId) -> Option<RenderTarget> {
        let running = self.running.as_ref()?;
        let shared = running.shared.borrow();
        let viewport_id = *shared.viewport_from_window.get(&window_id)?;
        let viewport = shared.viewports.get(&viewport_id)?;
        let window = viewport.window.as_ref()?;
        Some(RenderTarget {
            window_id,
            viewport_id,
            surface_generation: viewport.surface_generation,
            size: window.inner_size(),
        })
    }

    fn save(&mut self) {
        log::debug!("WinitApp::save called");
        if let Some(running) = self.running.as_mut() {
            running.save();
        }
    }

    fn save_and_destroy(&mut self) {
        if let Some(mut running) = self.running.take() {
            running.save_and_destroy();
        }
    }

    fn run_ui_and_paint(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        mode: PassMode,
    ) -> Result<EventResult> {
        self.initialized_all_windows(event_loop);

        if let Some(running) = &mut self.running {
            running.run_ui_and_paint(window_id, event_loop, mode)
        } else {
            Ok(EventResult::Wait)
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) -> crate::Result<EventResult> {
        log::debug!("Event::Resumed");

        let running = if let Some(running) = &self.running {
            #[cfg(target_os = "android")]
            self.recreate_window(event_loop, running);
            running
        } else {
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
                .unwrap_or_else(|| winit_integration::create_egui_context(storage.as_deref()));
            let (window, builder) = create_window(
                &egui_ctx,
                event_loop,
                storage.as_deref(),
                &mut self.native_options,
            )?;
            self.init_run_state(egui_ctx, event_loop, storage, window, builder)?
        };

        let viewport = &running.shared.borrow().viewports[&ViewportId::ROOT];
        if let Some(window) = &viewport.window {
            Ok(EventResult::repaint_now(
                window.id(),
                RepaintNowReason::Bootstrap,
            ))
        } else {
            Ok(EventResult::Wait)
        }
    }

    fn suspended(&mut self, _: &ActiveEventLoop) -> crate::Result<EventResult> {
        #[cfg(target_os = "android")]
        self.drop_window()?;
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
            let mut shared = running.shared.borrow_mut();
            let focused_viewport = shared.focused_viewport;
            let viewports = &mut shared.viewports;
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
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: winit::event::WindowEvent,
    ) -> crate::Result<EventResult> {
        self.initialized_all_windows(event_loop);

        if let Some(running) = &mut self.running {
            Ok(running.on_window_event(window_id, &event))
        } else {
            // running is removed to get ready for exiting
            Ok(EventResult::Exit)
        }
    }

    #[cfg(feature = "accesskit")]
    fn on_accesskit_event(&mut self, event: accesskit_winit::Event) -> crate::Result<EventResult> {
        if let Some(running) = &mut self.running {
            let mut shared_lock = running.shared.borrow_mut();
            let SharedState {
                viewport_from_window,
                viewports,
                ..
            } = &mut *shared_lock;
            if let Some(viewport) = viewport_from_window
                .get(&event.window_id)
                .and_then(|id| viewports.get_mut(id))
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

impl WgpuWinitRunning<'_> {
    /// Saves the application state
    fn save(&mut self) {
        let shared = self.shared.borrow();
        // This is done because of the "save on suspend" logic on Android. Once the application is suspended, there is no window associated to it.
        let window = if let Some(Viewport { window, .. }) = shared.viewports.get(&ViewportId::ROOT)
        {
            window.as_deref()
        } else {
            None
        };
        self.integration.save(self.app.as_mut(), window);
    }

    fn save_and_destroy(&mut self) {
        profiling::function_scope!();

        let ctx = self.integration.egui_ctx.clone();
        super::viewport_lifecycle::finish_pending_retirements(&ctx, &mut self.shared);
        ctx.on_exit();
        self.save();

        #[cfg(feature = "glow")]
        self.app.on_exit(None);

        #[cfg(not(feature = "glow"))]
        self.app.on_exit();

        super::viewport_lifecycle::finish_pending_retirements(&ctx, &mut self.shared);
        let mut shared = self.shared.borrow_mut();
        shared.painter.destroy();
    }

    /// This is called both for the root viewport, and all deferred viewports.
    fn run_ui_and_paint(
        &mut self,
        window_id: WindowId,
        event_loop: &ActiveEventLoop,
        mode: PassMode,
    ) -> Result<EventResult> {
        profiling::function_scope!();

        let Some(viewport_id) = self
            .shared
            .borrow()
            .viewport_from_window
            .get(&window_id)
            .copied()
        else {
            return Ok(EventResult::Wait);
        };

        profiling::finish_frame!();

        let Self {
            app,
            integration,
            shared,
        } = self;

        // Close is a lifecycle request, independent of taking input or running UI.
        super::viewport_lifecycle::dispatch_child_close_requests(
            &integration.egui_ctx,
            app.as_mut(),
            shared,
        );

        let mut frame_timer = crate::stopwatch::Stopwatch::new();
        frame_timer.start();

        let (viewport_ui_cb, raw_input, show_ui) = {
            profiling::scope!("Prepare");
            let mut shared_lock = shared.borrow_mut();

            let SharedState {
                viewports, painter, ..
            } = &mut *shared_lock;

            if viewport_id != ViewportId::ROOT {
                let Some(viewport) = viewports.get(&viewport_id) else {
                    return Ok(EventResult::Wait);
                };

                if viewport.viewport_ui_cb.is_none() {
                    // This will only happen if this is an immediate viewport.
                    // That means that the viewport cannot be rendered by itself and needs his parent to be rendered.
                    if let Some(viewport) = viewports.get(&viewport.ids.parent)
                        && let Some(window) = viewport.window.as_ref()
                    {
                        return Ok(EventResult::RepaintNext(window.id()));
                    }
                    return Ok(EventResult::Wait);
                }
            }

            let Some(viewport) = viewports.get_mut(&viewport_id) else {
                return Ok(EventResult::Wait);
            };

            let Some(window) = viewport.window.as_ref() else {
                return Ok(EventResult::Wait);
            };
            egui_winit::update_viewport_info(
                &mut viewport.info,
                &integration.egui_ctx,
                window,
                false,
            );

            let viewport = &viewports[&viewport_id];

            // UI progress and surface rendering have different qualifications.
            // In particular, Windows minimization does not skip a requested Full.
            let show_ui = super::frame_output::PassDecision::for_viewport(
                mode,
                &viewport.info,
                is_viewport_or_descendant_visible(viewports, viewport_id),
                viewport
                    .actions_requested
                    .iter()
                    .any(ActionRequested::wants_paint),
                cfg!(target_os = "windows"),
            )
            .run_ui;

            let Viewport {
                viewport_ui_cb,
                window,
                egui_winit,
                pending_raw_input,
                ..
            } = viewports
                .get_mut(&viewport_id)
                .expect("viewport was just found");
            let viewport_ui_cb = viewport_ui_cb.clone();
            let window = window.as_ref().expect("window was just found");

            {
                profiling::scope!("set_window");
                pollster::block_on(painter.set_window(viewport_id, Some(Arc::clone(window))))?;
            }

            let Some(egui_winit) = egui_winit.as_mut() else {
                return Ok(EventResult::Wait);
            };
            // All viewports share one font atlas; differing limits rebuild it per pass.
            if let Some(max_texture_side) = painter.max_texture_side() {
                egui_winit.set_max_texture_side(max_texture_side);
            }
            // A skipped child has no logic consumer: leave its State untouched.
            let mut raw_input = super::epi_integration::take_viewport_input_if(
                show_ui || viewport_id == ViewportId::ROOT,
                pending_raw_input,
                || egui_winit.take_egui_input(window),
            );

            integration.pre_update();

            if let Some(raw_input) = raw_input.as_mut() {
                raw_input.time = Some(integration.beginning.elapsed().as_secs_f64());
                raw_input.viewports = viewports
                    .iter()
                    .map(|(id, viewport)| (*id, viewport.info.clone()))
                    .collect();
            }

            #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
            if crate::miv_test_script_window_witness::capture_probe_detail_allowed() {
                egui_wgpu::atlas_diag::log_line(format!(
                    "[capture-probe] eframe_pass viewport={viewport_id:?} class=root_or_deferred"
                ));
            }
            (viewport_ui_cb, raw_input, show_ui)
        };

        if !show_ui {
            // Nothing will be shown, so we run no egui pass at all.
            // That way all ui state is left untouched, and is still there
            // when this viewport becomes visible again.
            let is_root_viewport = viewport_ui_cb.is_none();
            if is_root_viewport {
                // The app logic keeps ticking, so it can e.g. ask to be shown again:
                let (mut logic_output, unused_input) = integration.update_logic_only(
                    app.as_mut(),
                    raw_input.expect("root input is taken even without a UI pass"),
                );

                super::viewport_lifecycle::finish_logic_output(
                    &integration.egui_ctx,
                    shared,
                    &mut logic_output,
                );
                let egui::LogicOutput {
                    platform_output,
                    viewport_commands,
                } = logic_output;
                let mut shared_mut = shared.borrow_mut();
                let SharedState { viewports, .. } = &mut *shared_mut;

                if let Some(viewport) = viewports.get_mut(&viewport_id) {
                    super::epi_integration::finish_root_logic_input(
                        &mut viewport.pending_raw_input,
                        &mut viewport.info,
                        unused_input,
                    );
                    if let Viewport {
                        window: Some(window),
                        egui_winit: Some(egui_winit),
                        ..
                    } = viewport
                    {
                        egui_winit.handle_platform_output_with_event_loop(
                            window,
                            event_loop,
                            platform_output,
                        );
                    }
                }

                for (id, commands) in viewport_commands {
                    if let Some(viewport) = viewports.get_mut(&id) {
                        viewport.process_commands(&integration.egui_ctx, commands);
                    }
                }
            }

            super::viewport_lifecycle::dispatch_child_close_requests(
                &integration.egui_ctx,
                app.as_mut(),
                shared,
            );

            sleep_if_invisible_or_minimized(
                shared
                    .borrow()
                    .viewports
                    .get(&viewport_id)
                    .and_then(|viewport| viewport.window.as_deref()),
            );

            return Ok(if integration.should_close() {
                EventResult::CloseRequested
            } else {
                EventResult::Wait
            });
        }

        // ------------------------------------------------------------

        // Runs the update, which could call immediate viewports,
        // so make sure we hold no locks here!
        #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
        let callback_window = shared
            .borrow()
            .viewports
            .get(&viewport_id)
            .and_then(|viewport| viewport.window.clone());
        let _discard_texture_scope = enter_discard_texture_delivery(&integration.egui_ctx, shared);
        let mut full_output = {
            #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
            let _window_witness_scope = crate::miv_test_script_window_witness::enter_native_window(
                &integration.egui_ctx,
                viewport_id,
                callback_window.as_ref(),
            );
            #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
            drop(callback_window);
            integration.update(
                app.as_mut(),
                viewport_ui_cb.as_deref(),
                raw_input.expect("a Full UI pass takes its viewport's input"),
            )
        };
        egui_wgpu::atlas_diag::record_produced(
            egui_wgpu::atlas_diag::Site::ProducedMain,
            &full_output.textures_delta,
            Some(viewport_id.0.value()),
            egui_installed_atlas_size(&integration.egui_ctx),
        );
        super::viewport_lifecycle::append_texture_delta(
            &mut shared.borrow_mut().pending_deltas,
            core::mem::take(&mut full_output.textures_delta),
        );
        super::viewport_lifecycle::finish_full_output(
            &integration.egui_ctx,
            shared,
            &mut full_output,
        );
        let startup_tag = (viewport_id == ViewportId::ROOT)
            .then(|| egui_wgpu::take_root_startup_frame_tag(&integration.egui_ctx));

        // ------------------------------------------------------------

        let mut shared_mut = shared.borrow_mut();

        let SharedState {
            egui_ctx,
            viewports,
            painter,
            viewport_from_window,
            pending_deltas,
            ..
        } = &mut *shared_mut;

        let FullOutput {
            platform_output,
            textures_delta,
            shapes,
            pixels_per_point,
            mut viewport_output,
        } = full_output;

        super::viewport_lifecycle::append_texture_delta(pending_deltas, textures_delta);

        // Synchronization between the integration layer and the painter layer
        if let Some(surface_config) = integration.frame.wgpu_surface_config {
            painter.set_surface_config(surface_config);
        }

        // Completing this output never depends on a live surface. In particular,
        // a source viewport disappearing must not skip other viewports' commands.
        let vsync_secs = if let Some(viewport) = viewports.get_mut(&viewport_id) {
            viewport.info.events.clear(); // they should have been processed
            if let Viewport {
                window: Some(window),
                egui_winit: Some(egui_winit),
                info,
                actions_requested,
                ..
            } = viewport
            {
                let commands = &mut viewport_output
                    .get_mut(&viewport_id)
                    .expect("a retained viewport has an output entry")
                    .commands;
                super::frame_output::finish_main_viewport_output(
                    platform_output,
                    commands,
                    actions_requested,
                    &mut super::frame_output::WinitOutputTarget {
                        state: egui_winit,
                        window,
                        event_loop,
                    },
                );
                let paint = super::frame_output::PassDecision::for_viewport(
                    PassMode::Full,
                    info,
                    false,
                    actions_requested.iter().any(ActionRequested::wants_paint),
                    cfg!(target_os = "windows"),
                )
                .paint;
                finish_wgpu_main_render(
                    paint,
                    painter,
                    pending_deltas,
                    |painter, pending_deltas| {
                        let clipped_primitives = egui_ctx.tessellate(shapes, pixels_per_point);

                        let screenshot_commands = take_screenshot_requests(&mut *actions_requested);
                        #[cfg(all(
                            target_os = "windows",
                            feature = "miv-test-script-window-witness"
                        ))]
                        let screenshot_commands_count = screenshot_commands.len();
                        #[cfg(all(
                            target_os = "windows",
                            feature = "miv-test-script-window-witness"
                        ))]
                        if crate::miv_test_script_window_witness::capture_probe_detail_allowed() {
                            egui_wgpu::atlas_diag::log_line(format!(
                                "[capture-probe] painter_request viewport={viewport_id:?} class=root_or_deferred count={}",
                                screenshot_commands.len()
                            ));
                        }
                        #[cfg(all(test, target_os = "windows"))]
                        render_phase_test_gate::before_surface_acquire();
                        let vsync_secs = painter.paint_and_update_textures_with_startup_tag(
                            viewport_id,
                            pixels_per_point,
                            app.clear_color(&egui_ctx.global_style().visuals),
                            &clipped_primitives,
                            pending_deltas,
                            screenshot_commands,
                            window,
                            startup_tag,
                        );
                        #[cfg(all(
                            target_os = "windows",
                            feature = "miv-test-script-window-witness"
                        ))]
                        if crate::miv_test_script_window_witness::capture_probe_detail_allowed() {
                            egui_wgpu::atlas_diag::log_line(format!(
                                "[capture-probe] painter_return viewport={viewport_id:?} class=root_or_deferred request_count={} image_delivery=async",
                                screenshot_commands_count
                            ));
                        }
                        #[cfg(all(test, target_os = "windows"))]
                        render_phase_test_gate::after_paint();

                        actions_requested
                            .retain(|action| !matches!(action, ActionRequested::PaintWhileHidden));
                        integration.post_rendering(window);
                        vsync_secs
                    },
                    |painter, pending_deltas| painter.apply_textures_delta(pending_deltas),
                )
            } else {
                painter.apply_textures_delta(pending_deltas);
                0.0
            }
        } else {
            painter.apply_textures_delta(pending_deltas);
            0.0
        };

        handle_viewport_output(
            &integration.egui_ctx,
            &viewport_output,
            viewports,
            painter,
            viewport_from_window,
        );

        let window = viewport_from_window
            .get(&window_id)
            .and_then(|id| viewports.get(id))
            .and_then(|vp| vp.window.as_ref());

        integration.report_frame_time(frame_timer.total_time_sec() - vsync_secs); // don't count auto-save time as part of regular frame time

        let window_for_autosave = if viewport_id == ViewportId::ROOT {
            window.map(|window| window.as_ref())
        } else {
            None
        };
        integration.maybe_autosave(app.as_mut(), window_for_autosave);

        sleep_if_invisible_or_minimized(window.map(|window| window.as_ref()));

        // Immediate children can queue Close while the outer pass is active.
        // Output and shared deltas have been delivered before retirement.
        drop(shared_mut);
        super::viewport_lifecycle::dispatch_child_close_requests(
            &integration.egui_ctx,
            app.as_mut(),
            shared,
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
        let Self {
            app,
            integration,
            shared: shared_owner,
        } = self;
        let mut shared = shared_owner.borrow_mut();

        let viewport_id = shared.viewport_from_window.get(&window_id).copied();

        // A renderer may be entered from message dispatch only for an InteractiveResize frame
        // responding to this window's non-zero client resize. This decision uses event provenance
        // only: never timing, inferred geometry, focus, VST, or detached predicates. Every other
        // redraw, bootstrap, AccessKit request, and hidden-window frame is owned by the outer render
        // phase. An inline resize frame's immediate-viewport render subtree remains part of that
        // frame, rather than a separate scheduler re-entry.
        //
        // On Windows, if a window is resized by the user, it should repaint synchronously, inside the
        // event handler. If this is not done, the compositor will assume that the window does not want
        // to redraw and continue ahead.
        //
        // In eframe's case, that causes the window to rapidly flicker, as it struggles to deliver
        // new frames to the compositor in time. The flickering is technically glutin or glow's fault, but we should be responding properly
        // to resizes anyway, as doing so avoids dropping frames.
        //
        // See: https://github.com/emilk/egui/issues/903
        let mut repaint_asap = false;

        // On MacOS the asap repaint is not enough. The drawn frames must be synchronized with
        // the CoreAnimation transactions driving the window resize process.
        //
        // Thus, Painter, responsible for wgpu surfaces and their resize, has to be notified of the
        // resize lifecycle, yet winit does not provide any events for that. To work around,
        // the last resized viewport is tracked until a later event outside the live resize stream
        // is received.
        //
        // AppKit can emit `Moved` events during top/left live resize because the window origin
        // changes along with the content size. Treat those as part of live resize on macOS.
        //
        // See: https://github.com/emilk/egui/issues/903
        let event_keeps_resize_active = matches!(event, winit::event::WindowEvent::Resized(_))
            || (cfg!(target_os = "macos") && matches!(event, winit::event::WindowEvent::Moved(_)));

        if !event_keeps_resize_active
            && let Some(id) = viewport_id
            && shared.resized_viewport == viewport_id
        {
            shared.painter.on_window_resize_state_change(id, false);
            shared.resized_viewport = None;
        }

        match event {
            winit::event::WindowEvent::Focused(focused) => {
                let focused = if cfg!(target_os = "macos")
                    && let Some(viewport_id) = viewport_id
                    && let Some(viewport) = shared.viewports.get(&viewport_id)
                    && let Some(window) = &viewport.window
                {
                    // TODO(emilk): remove this work-around once we update winit
                    // https://github.com/rust-windowing/winit/issues/4371
                    // https://github.com/emilk/egui/issues/7588
                    window.has_focus()
                } else {
                    *focused
                };

                shared.focused_viewport = focused.then_some(viewport_id).flatten();
            }

            // The reply to `Frame::request_activation_token`. Hold it for the app
            // to take, and repaint: a token is short-lived and single-use, so an
            // idle app must not have to wait for an unrelated event to notice it.
            winit::event::WindowEvent::ActivationTokenDone { token, .. } => {
                integration.frame.activation_token = Some(token.clone().into_raw());
                if let Some(viewport_id) = viewport_id {
                    integration.egui_ctx.request_repaint_of(viewport_id);
                } else {
                    integration.egui_ctx.request_repaint();
                }
            }

            winit::event::WindowEvent::Resized(physical_size) => {
                let nonzero_resize_event = physical_size.width > 0 && physical_size.height > 0;
                // The event can be stale: winit buffers events emitted while we create and set up
                // the window (e.g. maximize it), so it can arrive after the window has changed size again.
                let physical_size = viewport_id
                    .and_then(|id| shared.viewports.get(&id))
                    .and_then(|viewport| viewport.window.as_ref())
                    .map_or(*physical_size, |window| window.inner_size());

                // Resize with 0 width and height is used by winit to signal a minimize event on Windows.
                // See: https://github.com/rust-windowing/winit/issues/208
                // This solves an issue where the app would panic when minimizing on Windows.
                if let Some(id) = viewport_id
                    && let (Some(width), Some(height)) = (
                        NonZeroU32::new(physical_size.width),
                        NonZeroU32::new(physical_size.height),
                    )
                {
                    if shared.resized_viewport != viewport_id {
                        shared.resized_viewport = viewport_id;
                        shared.painter.on_window_resize_state_change(id, true);
                    }
                    shared.painter.on_window_resized(id, width, height);
                    if let Some(viewport) = shared.viewports.get_mut(&id) {
                        viewport.surface_generation =
                            viewport.surface_generation.wrapping_add(1).max(1);
                    }
                    // Geometry refresh must not turn a zero-size/minimize event into an
                    // inline render request. The exception belongs to the received event.
                    repaint_asap = nonzero_resize_event;
                }
            }

            winit::event::WindowEvent::Occluded(is_occluded) => {
                if let Some(viewport_id) = viewport_id
                    && let Some(viewport) = shared.viewports.get_mut(&viewport_id)
                {
                    viewport.info.occluded = Some(*is_occluded);
                }
            }

            winit::event::WindowEvent::CloseRequested => {
                if viewport_id == Some(ViewportId::ROOT) && integration.should_close() {
                    log::debug!(
                        "Received WindowEvent::CloseRequested for main viewport - shutting down."
                    );
                    return EventResult::CloseRequested;
                }

                log::debug!("Received WindowEvent::CloseRequested for viewport {viewport_id:?}");

                if let Some(viewport_id) = viewport_id
                    && let Some(viewport) = shared.viewports.get_mut(&viewport_id)
                {
                    // Tell viewport it should close:
                    viewport.info.events.push(egui::ViewportEvent::Close);
                }
            }

            _ => {}
        }

        let event_response = viewport_id
            .and_then(|viewport_id| {
                let viewport = shared.viewports.get_mut(&viewport_id)?;
                Some(integration.on_window_event(
                    viewport.window.as_deref()?,
                    viewport.egui_winit.as_mut()?,
                    event,
                ))
            })
            .unwrap_or_default();

        drop(shared);
        super::viewport_lifecycle::dispatch_child_close_requests(
            &integration.egui_ctx,
            app.as_mut(),
            shared_owner,
        );
        let shared = shared_owner.borrow();
        if matches!(event, winit::event::WindowEvent::CloseRequested)
            && let Some(viewport) = viewport_id.and_then(|id| shared.viewports.get(&id))
            && viewport.info.close_requested()
        {
            // Preserve the traditional UI-delegated close path, after lifecycle handling.
            integration.egui_ctx.request_repaint_of(viewport.ids.this);
            integration.egui_ctx.request_repaint_of(viewport.ids.parent);
        }

        if integration.should_close() {
            EventResult::CloseRequested
        } else if !shared.viewport_from_window.contains_key(&window_id) {
            EventResult::Wait
        } else if event_response.repaint {
            if repaint_asap {
                EventResult::repaint_now(window_id, RepaintNowReason::InteractiveResize)
            } else {
                EventResult::RepaintNext(window_id)
            }
        } else {
            EventResult::Wait
        }
    }
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
        // Close must be observable even before a native window is initialized.
        commands.retain(|command| {
            if matches!(command, egui::ViewportCommand::Close) {
                self.info.events.push(egui::ViewportEvent::Close);
                false
            } else {
                true
            }
        });
        self.deferred_commands.append(&mut commands);

        if let Some(window) = self.window.as_ref() {
            #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
            let focus_commands = self
                .deferred_commands
                .iter()
                .filter(|command| {
                    matches!(
                        command,
                        egui::ViewportCommand::Focus
                            | egui::ViewportCommand::Visible(_)
                            | egui::ViewportCommand::WindowLevel(_)
                    )
                })
                .map(|command| format!("{command:?}"))
                .collect::<Vec<_>>();
            #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
            if !focus_commands.is_empty() {
                egui_wgpu::atlas_diag::log_line(format!(
                    "[focus-probe] before viewport={:?} commands={focus_commands:?} has_focus={}",
                    self.ids.this,
                    window.has_focus()
                ));
            }
            egui_winit::process_viewport_commands(
                egui_ctx,
                &mut self.info,
                core::mem::take(&mut self.deferred_commands),
                window,
                &mut self.actions_requested,
            );
            #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
            if !focus_commands.is_empty() {
                egui_wgpu::atlas_diag::log_line(format!(
                    "[focus-probe] after viewport={:?} has_focus={}",
                    self.ids.this,
                    window.has_focus()
                ));
            }
            #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
            if crate::miv_test_script_window_witness::capture_probe_detail_allowed() {
                let queued = self
                    .actions_requested
                    .iter()
                    .filter(|action| matches!(action, ActionRequested::Screenshot(_)))
                    .count();
                egui_wgpu::atlas_diag::log_line(format!(
                    "[capture-probe] eframe_queued viewport={:?} screenshot_actions={queued} native_window=true",
                    self.ids.this
                ));
            }
        }
    }

    /// Create winit window, if needed.
    fn initialize_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        egui_ctx: &egui::Context,
        windows_id: &mut HashMap<WindowId, ViewportId>,
        painter: &mut egui_wgpu::winit::Painter,
        clipboard_shortcuts: bool,
    ) {
        if self.window.is_some() {
            return; // we already have one
        }

        profiling::function_scope!();

        let viewport_id = self.ids.this;

        match egui_winit::create_window(egui_ctx, event_loop, &self.builder) {
            Ok(window) => {
                windows_id.insert(window.id(), viewport_id);

                let window = Arc::new(window);

                if let Err(err) =
                    pollster::block_on(painter.set_window(viewport_id, Some(Arc::clone(&window))))
                {
                    log::error!("on set_window: viewport_id {viewport_id:?} {err}");
                }

                let mut egui_winit = egui_winit::State::new(
                    egui_ctx.clone(),
                    viewport_id,
                    event_loop,
                    Some(window.scale_factor() as f32),
                    event_loop.system_theme(),
                    painter.max_texture_side(),
                );
                egui_winit.set_clipboard_shortcuts(clipboard_shortcuts);
                self.egui_winit = Some(egui_winit);

                egui_winit::update_viewport_info(&mut self.info, egui_ctx, &window, true);
                self.surface_generation = self.surface_generation.wrapping_add(1).max(1);
                self.window = Some(window);
            }
            Err(err) => {
                log::error!("Failed to create window: {err}");
            }
        }
    }
}

fn create_window(
    egui_ctx: &egui::Context,
    event_loop: &ActiveEventLoop,
    storage: Option<&dyn Storage>,
    native_options: &mut NativeOptions,
) -> Result<(Window, ViewportBuilder), winit::error::OsError> {
    profiling::function_scope!();

    let mut window_settings = epi_integration::load_window_settings(storage);
    let viewport_builder = epi_integration::viewport_builder(
        egui_ctx.zoom_factor(),
        event_loop,
        native_options,
        &mut window_settings,
    )
    .with_visible(false); // Start hidden until we render the first frame to fix white flash on startup (https://github.com/emilk/egui/pull/3631)

    if let Some(callback) = &native_options.wgpu_options.startup_diagnostics {
        callback(egui_wgpu::StartupDiagnosticEvent::Begin(
            egui_wgpu::StartupGpuStage::WindowCreate,
        ));
    }
    let result = egui_winit::create_window(egui_ctx, event_loop, &viewport_builder);
    if let Some(callback) = &native_options.wgpu_options.startup_diagnostics {
        callback(egui_wgpu::StartupDiagnosticEvent::End {
            stage: egui_wgpu::StartupGpuStage::WindowCreate,
            success: result.is_ok(),
        });
    }
    let window = result?;
    epi_integration::apply_window_settings(&window, window_settings);
    Ok((window, viewport_builder))
}

/// Is this viewport, or any of its (transitive) descendant viewports, visible?
///
/// Immediate viewports are rendered inline while their parent's UI runs, so even
/// if this viewport's window is occluded or minimized we must still run its UI to
/// give any visible descendant a chance to be painted.
fn is_viewport_or_descendant_visible(viewports: &Viewports, viewport_id: ViewportId) -> bool {
    let Some(viewport) = viewports.get(&viewport_id) else {
        return false;
    };
    if viewport.info.visible().unwrap_or(true) {
        return true;
    }
    viewports.values().any(|child| {
        child.ids.parent == viewport_id
            && child.ids.this != viewport_id
            && is_viewport_or_descendant_visible(viewports, child.ids.this)
    })
}

fn render_immediate_viewport(
    backend: &mut impl super::frame_output::ImmediateViewportBackend,
    immediate_viewport: ImmediateViewport<'_>,
) {
    profiling::function_scope!();
    super::frame_output::run_immediate_viewport(backend, immediate_viewport);
}

#[derive(Default)]
struct ImmediateWgpuTimings {
    input_max_tex: Option<usize>,
    input_ms: f64,
    run_ms: f64,
    set_window_ms: f64,
    tessellate_ms: f64,
    paint_ms: f64,
    resource_ms: f64,
    platform_ms: f64,
    viewport_out_ms: f64,
    delta_set: usize,
    delta_free: usize,
    delta_bytes: usize,
    primitive_count: usize,
}

struct NativeWgpuImmediateBackend<'a> {
    beginning: Instant,
    shared: &'a RefCell<SharedState>,
    started: Instant,
    timings: ImmediateWgpuTimings,
}

impl<'a> NativeWgpuImmediateBackend<'a> {
    fn new(beginning: Instant, shared: &'a RefCell<SharedState>) -> Self {
        Self {
            beginning,
            shared,
            started: Instant::now(),
            timings: Default::default(),
        }
    }
}

impl super::frame_output::ImmediateViewportBackend for NativeWgpuImmediateBackend<'_> {
    fn prepare(
        &mut self,
        ids: ViewportIdPair,
        builder: ViewportBuilder,
    ) -> Option<(egui::Context, egui::RawInput)> {
        profiling::function_scope!();
        let ctx = self.shared.borrow().egui_ctx.clone();
        assert!(
            ctx.has_discard_texture_delivery(),
            "native immediate requires its Full resource scope"
        );
        replace_stale_wgpu_viewport(self.shared, &ctx, ids.this);
        let SharedState {
            egui_ctx,
            viewports,
            painter,
            viewport_from_window,
            clipboard_shortcuts,
            ..
        } = &mut *self.shared.borrow_mut();
        let viewport = initialize_or_update_viewport(
            viewports,
            ids,
            ViewportClass::Immediate,
            builder,
            None,
            painter,
        );
        viewport.registration = egui_ctx.viewport_registration(ids.this);
        viewport.parent_registration = egui_ctx.viewport_registration(ids.parent);
        if viewport.window.is_none() {
            event_loop_context::with_current_event_loop(|event_loop| {
                viewport.initialize_window(
                    event_loop,
                    egui_ctx,
                    viewport_from_window,
                    painter,
                    *clipboard_shortcuts,
                );
            });
        }
        let (Some(window), Some(state)) = (&viewport.window, &mut viewport.egui_winit) else {
            return None;
        };
        egui_winit::update_viewport_info(&mut viewport.info, egui_ctx, window, false);
        // All viewports use the same renderer/font atlas limit.
        if let Some(max_texture_side) = painter.max_texture_side() {
            state.set_max_texture_side(max_texture_side);
        }
        let mut input =
            super::epi_integration::take_viewport_input(&mut viewport.pending_raw_input, || {
                state.take_egui_input(window)
            });
        input.viewports = viewports
            .iter()
            .map(|(id, viewport)| (*id, viewport.info.clone()))
            .collect();
        input.time = Some(self.beginning.elapsed().as_secs_f64());
        self.timings.input_max_tex = input.max_texture_side;
        self.timings.input_ms = self.started.elapsed().as_secs_f64() * 1000.0;
        Some((egui_ctx.clone(), input))
    }

    fn run_ui(
        &mut self,
        ids: ViewportIdPair,
        ctx: &egui::Context,
        input: egui::RawInput,
        callback: &mut dyn FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        let started = Instant::now();
        #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
        if crate::miv_test_script_window_witness::capture_probe_detail_allowed() {
            egui_wgpu::atlas_diag::log_line(format!(
                "[capture-probe] eframe_pass viewport={:?} class=immediate",
                ids.this
            ));
        }
        #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
        let callback_window = self
            .shared
            .borrow()
            .viewports
            .get(&ids.this)
            .and_then(|viewport| viewport.window.clone());
        #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
        let _window_witness_scope = crate::miv_test_script_window_witness::enter_native_window(
            ctx,
            ids.this,
            callback_window.as_ref(),
        );
        #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
        drop(callback_window);
        let _ = ids;
        // No backend borrow is held: this callback may recursively open a child.
        let output = ctx.run_ui(input, callback);
        self.timings.run_ms = started.elapsed().as_secs_f64() * 1000.0;
        output
    }

    fn accept_textures(&mut self, ids: ViewportIdPair, ctx: &egui::Context, delta: TexturesDelta) {
        egui_wgpu::atlas_diag::record_produced(
            egui_wgpu::atlas_diag::Site::ProducedImmediate,
            &delta,
            Some(ids.this.0.value()),
            egui_installed_atlas_size(ctx),
        );
        (
            self.timings.delta_set,
            self.timings.delta_free,
            self.timings.delta_bytes,
        ) = texture_delta_stats(&delta);
        // Transfer before looking up the producing viewport; its lifetime may end in UI.
        super::viewport_lifecycle::append_texture_delta(
            &mut self.shared.borrow_mut().pending_deltas,
            delta,
        );
    }

    fn finish_output(
        &mut self,
        ids: ViewportIdPair,
        ctx: &egui::Context,
        platform: egui::PlatformOutput,
        outputs: &mut OrderedViewportIdMap<ViewportOutput>,
    ) -> Option<(ViewportInfo, bool)> {
        let started = Instant::now();
        let mut shared = self.shared.borrow_mut();
        let viewport = shared.viewports.get_mut(&ids.this)?;
        viewport.info.events.clear();
        let (Some(window), Some(state)) = (&viewport.window, &mut viewport.egui_winit) else {
            return None;
        };
        let output = outputs.get_mut(&ids.this)?;
        super::frame_output::finish_main_viewport_output(
            platform,
            &mut output.commands,
            &mut viewport.actions_requested,
            &mut super::frame_output::WinitImmediateOutputTarget { state, window },
        );
        egui_winit::update_viewport_info(&mut viewport.info, ctx, window, false);
        self.timings.platform_ms = started.elapsed().as_secs_f64() * 1000.0;
        Some((
            viewport.info.clone(),
            viewport
                .actions_requested
                .iter()
                .any(ActionRequested::wants_paint),
        ))
    }

    fn tessellate(
        &mut self,
        ctx: &egui::Context,
        shapes: Vec<egui::epaint::ClippedShape>,
        pixels_per_point: f32,
    ) -> Vec<egui::ClippedPrimitive> {
        let started = Instant::now();
        let primitives = ctx.tessellate(shapes, pixels_per_point);
        self.timings.tessellate_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.timings.primitive_count = primitives.len();
        primitives
    }

    fn paint(
        &mut self,
        ids: ViewportIdPair,
        _ctx: &egui::Context,
        primitives: Vec<egui::ClippedPrimitive>,
        pixels_per_point: f32,
    ) {
        let mut shared = self.shared.borrow_mut();
        let SharedState {
            viewports,
            painter,
            pending_deltas,
            ..
        } = &mut *shared;
        let Some(viewport) = viewports.get_mut(&ids.this) else {
            painter.apply_textures_delta(pending_deltas);
            return;
        };
        let Some(window) = viewport.window.as_ref() else {
            painter.apply_textures_delta(pending_deltas);
            return;
        };
        let started = Instant::now();
        {
            profiling::scope!("set_window");
            if let Err(err) =
                pollster::block_on(painter.set_window(ids.this, Some(Arc::clone(window))))
            {
                log::error!(
                    "when rendering viewport_id={:?}, set_window Error {err}",
                    ids.this
                );
                painter.apply_textures_delta(pending_deltas);
                return;
            }
        }
        self.timings.set_window_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = Instant::now();
        let screenshots = take_screenshot_requests(&mut viewport.actions_requested);
        #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
        let count = screenshots.len();
        #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
        if crate::miv_test_script_window_witness::capture_probe_detail_allowed() {
            egui_wgpu::atlas_diag::log_line(format!(
                "[capture-probe] painter_request viewport={:?} class=immediate count={count}",
                ids.this
            ));
        }
        painter.paint_and_update_textures(
            ids.this,
            pixels_per_point,
            [0.0; 4],
            &primitives,
            pending_deltas,
            screenshots,
            window,
        );
        viewport
            .actions_requested
            .retain(|action| !matches!(action, ActionRequested::PaintWhileHidden));
        #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
        if crate::miv_test_script_window_witness::capture_probe_detail_allowed() {
            egui_wgpu::atlas_diag::log_line(format!(
                "[capture-probe] painter_return viewport={:?} class=immediate request_count={count} image_delivery=async",
                ids.this
            ));
        }
        self.timings.paint_ms = started.elapsed().as_secs_f64() * 1000.0;
    }

    fn apply_resources(&mut self, _ids: ViewportIdPair, _ctx: &egui::Context) {
        let started = Instant::now();
        let mut shared = self.shared.borrow_mut();
        let SharedState {
            painter,
            pending_deltas,
            ..
        } = &mut *shared;
        painter.apply_textures_delta(pending_deltas);
        self.timings.resource_ms = started.elapsed().as_secs_f64() * 1000.0;
    }

    fn finish_viewports(
        &mut self,
        _ids: ViewportIdPair,
        ctx: &egui::Context,
        outputs: OrderedViewportIdMap<ViewportOutput>,
    ) {
        let started = Instant::now();
        let SharedState {
            viewports,
            painter,
            viewport_from_window,
            ..
        } = &mut *self.shared.borrow_mut();
        handle_viewport_output(ctx, &outputs, viewports, painter, viewport_from_window);
        self.timings.viewport_out_ms = started.elapsed().as_secs_f64() * 1000.0;
    }

    fn finished(&mut self, ids: ViewportIdPair, _ctx: &egui::Context) {
        let total_ms = self.started.elapsed().as_secs_f64() * 1000.0;
        let m = &self.timings;
        // Keep the ordinary (non-feature-gated) slow immediate delta witness.
        if total_ms >= 8.0 && m.delta_bytes > 0 {
            egui_wgpu::atlas_diag::log_line(format!(
                "[eframe] immediate_viewport id={} total={total_ms:.1}ms input={:.1} run={:.1} set_window={:.1} tessellate={:.1} paint={:.1} resource={:.1} platform_out={:.1} viewport_out={:.1} prims={} delta_set={} delta_free={} delta_bytes={} max_tex={:?}",
                ids.this.0.value(),
                m.input_ms,
                m.run_ms,
                m.set_window_ms,
                m.tessellate_ms,
                m.paint_ms,
                m.resource_ms,
                m.platform_ms,
                m.viewport_out_ms,
                m.primitive_count,
                m.delta_set,
                m.delta_free,
                m.delta_bytes,
                m.input_max_tex
            ));
        }
    }
}

fn texture_delta_stats(delta: &TexturesDelta) -> (usize, usize, usize) {
    let update_count = delta.set.values().map(|updates| updates.len()).sum();
    let bytes = delta
        .set
        .values()
        .flatten()
        .map(|delta| match &delta.image {
            egui::ImageData::Color(image) => image.pixels.len() * 4,
        })
        .sum();
    (update_count, delta.free.len(), bytes)
}

fn take_screenshot_requests(actions: &mut Vec<ActionRequested>) -> Vec<egui::ScreenshotCallback> {
    let mut screenshots = Vec::new();
    actions.retain(|action| {
        if let ActionRequested::Screenshot(data) = action {
            screenshots.push(data.clone());
            false
        } else {
            true
        }
    });
    screenshots
}

/// The main pass always delivers resources; only the qualified surface path
/// may tessellate, render, capture or publish a visible commit.
fn finish_wgpu_main_render<P>(
    paint: bool,
    painter: &mut P,
    pending: &mut TexturesDelta,
    render: impl FnOnce(&mut P, &mut TexturesDelta) -> f32,
    apply_resources: impl FnOnce(&mut P, &mut TexturesDelta),
) -> f32 {
    if paint {
        render(painter, pending)
    } else {
        apply_resources(painter, pending);
        0.0
    }
}

/// mIV: what egui's own texture manager believes is installed for the font atlas.
///
/// This is the value the backend is obliged to have on the GPU once it has applied the batch that
/// was just produced, so a dump can say whether the renderer fell behind egui or egui never
/// emitted the growth in the first place. See `egui_wgpu::atlas_diag`.
fn egui_installed_atlas_size(egui_ctx: &egui::Context) -> Option<[usize; 2]> {
    let tex_manager = egui_ctx.tex_manager();
    let guard = tex_manager.read();
    guard.meta(egui::TextureId::default()).map(|meta| meta.size)
}

#[cfg(test)]
pub(crate) fn remove_viewports_not_in(
    viewports: &mut Viewports,
    painter: &mut egui_wgpu::winit::Painter,
    viewport_from_window: &mut HashMap<WindowId, ViewportId>,
    viewport_output: &OrderedViewportIdMap<ViewportOutput>,
) {
    let active_viewports_ids: ViewportIdSet = viewport_output.keys().copied().collect();

    retain_viewport_resources(
        viewports,
        viewport_from_window,
        &active_viewports_ids,
        |active| painter.gc_viewports(active),
    );
}

/// Both normal output GC and accepted lifecycle Close end the same native owners.
/// The surface owner is injectable so headless tests exercise production cleanup.
fn retain_viewport_resources<K: Eq + std::hash::Hash>(
    viewports: &mut Viewports,
    viewport_from_window: &mut HashMap<K, ViewportId>,
    active: &ViewportIdSet,
    mut retain_surfaces: impl FnMut(&ViewportIdSet),
) {
    viewports.retain(|id, _| active.contains(id));
    viewport_from_window.retain(|_, id| active.contains(id));
    retain_surfaces(active);
}

impl super::viewport_lifecycle::ChildCloseBackend for Rc<RefCell<SharedState>> {
    fn close_requests(&self) -> Vec<ViewportId> {
        self.borrow()
            .viewports
            .iter()
            .filter_map(|(id, viewport)| {
                (viewport.info.close_requested()
                    || viewport
                        .deferred_commands
                        .contains(&egui::ViewportCommand::Close))
                .then_some(*id)
            })
            .collect()
    }

    fn consume_close(&mut self, id: ViewportId) {
        let mut shared = self.borrow_mut();
        if let Some(viewport) = shared.viewports.get_mut(&id) {
            viewport
                .info
                .events
                .retain(|event| !matches!(event, egui::ViewportEvent::Close));
            viewport
                .deferred_commands
                .retain(|command| !matches!(command, egui::ViewportCommand::Close));
        }
        for viewport in shared.viewports.values_mut() {
            super::viewport_lifecycle::consume_close_in_input(&mut viewport.pending_raw_input, id);
            if let Some(state) = &mut viewport.egui_winit {
                super::viewport_lifecycle::consume_close_in_input(state.egui_input_mut(), id);
            }
        }
    }

    fn registration(
        &self,
        ctx: &egui::Context,
        id: ViewportId,
    ) -> Option<egui::ViewportRegistration> {
        self.borrow()
            .viewports
            .get(&id)
            .and_then(|vp| vp.registration)
            .or_else(|| ctx.viewport_registration(id))
    }

    fn retire(&mut self, ctx: &egui::Context, approval: &egui::ApprovedViewportRetirement) {
        let (approval, retired) = super::viewport_lifecycle::native_retirement_ids(
            ctx,
            approval,
            self.borrow()
                .viewports
                .iter()
                .map(|(id, vp)| (*id, vp.registration, vp.parent_registration)),
        );
        retire_wgpu_native_resources(self, &retired);
        super::viewport_lifecycle::finish_context_retirement(ctx, &approval);
    }

    fn gc(&mut self, ctx: &egui::Context) {
        let retired: ViewportIdSet = self
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
        retire_wgpu_native_resources(self, &retired);
        let ime_ids: Vec<_> = retired
            .into_iter()
            .filter(|id| ctx.viewport_registration(*id).is_none())
            .collect();
        egui_ime::retire_viewport_ime_policy(ctx, &ime_ids);
    }

    fn collect_retirement_delta(&mut self, ctx: &egui::Context) {
        super::viewport_lifecycle::append_texture_delta(
            &mut self.borrow_mut().pending_deltas,
            ctx.take_viewport_retirement_texture_delta(),
        );
    }

    fn finish_retirement(&mut self, ctx: &egui::Context) {
        super::viewport_lifecycle::ChildCloseBackend::collect_retirement_delta(self, ctx);
        let mut shared = self.borrow_mut();
        let SharedState {
            painter,
            pending_deltas,
            ..
        } = &mut *shared;
        painter.apply_textures_delta(pending_deltas);
    }
}

fn enter_discard_texture_delivery(
    ctx: &egui::Context,
    shared: &Rc<RefCell<SharedState>>,
) -> impl Drop + use<> {
    let shared = Rc::downgrade(shared);
    ctx.enter_discard_texture_delivery(move |delta| {
        let shared = shared
            .upgrade()
            .expect("native Full resource owner must outlive its scope");
        let mut shared = shared.borrow_mut();
        let SharedState {
            painter,
            pending_deltas,
            ..
        } = &mut *shared;
        super::viewport_lifecycle::append_texture_delta(pending_deltas, delta);
        if !pending_deltas.is_empty() {
            painter.apply_textures_delta(pending_deltas);
        }
    })
}

fn replace_stale_wgpu_viewport(shared: &RefCell<SharedState>, ctx: &egui::Context, id: ViewportId) {
    let stale = super::viewport_lifecycle::stale_native_subtree(
        ctx,
        id,
        shared
            .borrow()
            .viewports
            .iter()
            .map(|(id, vp)| (*id, vp.registration, vp.parent_registration)),
    );
    if !stale.is_empty() {
        retire_wgpu_native_resources(shared, &stale);
        // Native callback Drop may free textures while the parent UI pass is
        // paused. Transfer those deltas now; the existing renderer submission
        // still owns when sets/frees actually run.
        let delta = ctx.tex_manager().write().take_delta();
        super::viewport_lifecycle::append_texture_delta(
            &mut shared.borrow_mut().pending_deltas,
            delta,
        );
    }
}

fn retire_wgpu_native_resources(shared: &RefCell<SharedState>, retired: &ViewportIdSet) {
    let removed = {
        let mut shared = shared.borrow_mut();
        let SharedState {
            viewports,
            painter,
            viewport_from_window,
            focused_viewport,
            resized_viewport,
            ..
        } = &mut *shared;
        let order = super::viewport_lifecycle::child_first_ids(
            retired,
            viewports.iter().map(|(id, vp)| (*id, vp.ids.parent)),
        );
        let removed: Vec<_> = order
            .into_iter()
            .filter_map(|id| viewports.remove(&id))
            .collect();
        let active = viewports.keys().copied().collect();
        retain_viewport_resources(viewports, viewport_from_window, &active, |active| {
            painter.gc_viewports(active)
        });
        if focused_viewport.is_some_and(|id| retired.contains(&id)) {
            *focused_viewport = None;
        }
        if resized_viewport.is_some_and(|id| retired.contains(&id)) {
            *resized_viewport = None;
        }
        removed
    };
    // Deferred callbacks may call Context, or inspect native ownership in Drop.
    drop(removed);
}

#[cfg(test)]
fn native_viewport_subtree(viewports: &Viewports, root: ViewportId) -> ViewportIdSet {
    let mut retired = ViewportIdSet::from_iter([root]);
    loop {
        let before = retired.len();
        for (id, viewport) in viewports {
            if retired.contains(&viewport.ids.parent) {
                retired.insert(*id);
            }
        }
        if retired.len() == before {
            return retired;
        }
    }
}

/// Add new viewports, and update existing ones:
fn handle_viewport_output(
    egui_ctx: &egui::Context,
    viewport_output: &OrderedViewportIdMap<ViewportOutput>,
    viewports: &mut Viewports,
    painter: &mut egui_wgpu::winit::Painter,
    _viewport_from_window: &mut HashMap<WindowId, ViewportId>,
) {
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
            || viewports
                .get(&viewport_id)
                .is_some_and(|vp| vp.registration.is_some_and(|native| native != registration))
        {
            // A nested immediate output cannot retag another native lifetime.
            // The outer completed-output cleanup removes it before replacement.
            continue;
        }
        let ids = ViewportIdPair::from_self_and_parent(viewport_id, parent);

        let viewport =
            initialize_or_update_viewport(viewports, ids, class, builder, viewport_ui_cb, painter);

        #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
        if crate::miv_test_script_window_witness::capture_probe_detail_allowed() {
            let screenshot_count = commands
                .iter()
                .filter(|command| matches!(command, egui::ViewportCommand::Screenshot(_)))
                .count();
            let class_name = match class {
                ViewportClass::Root => "root",
                ViewportClass::Deferred => "deferred",
                ViewportClass::Immediate => "immediate",
                ViewportClass::EmbeddedWindow => "embedded",
            };
            egui_wgpu::atlas_diag::log_line(format!(
                "[capture-probe] eframe_output viewport={viewport_id:?} class={class_name} screenshot_commands={screenshot_count} native_window={} deferred_commands={}",
                viewport.window.is_some(),
                viewport.deferred_commands.len()
            ));
        }
        viewport.registration = Some(registration);
        viewport.parent_registration = parent_registration;
        let old_inner_size = viewport.window.as_ref().map(|window| window.inner_size());

        viewport.process_commands(egui_ctx, commands);

        #[cfg(all(target_os = "windows", feature = "miv-test-script-window-witness"))]
        if viewport.window.is_none()
            && crate::miv_test_script_window_witness::capture_probe_detail_allowed()
        {
            egui_wgpu::atlas_diag::log_line(format!(
                "[capture-probe] eframe_queued viewport={viewport_id:?} native_window=false deferred_commands={}",
                viewport.deferred_commands.len()
            ));
        }

        // For Wayland : https://github.com/emilk/egui/issues/4196
        if cfg!(target_os = "linux")
            && let Some(window) = viewport.window.as_ref()
            && let Some(old_inner_size) = old_inner_size
        {
            let new_inner_size = window.inner_size();
            if new_inner_size != old_inner_size
                && let (Some(width), Some(height)) = (
                    NonZeroU32::new(new_inner_size.width),
                    NonZeroU32::new(new_inner_size.height),
                )
            {
                painter.on_window_resized(viewport_id, width, height);
            }
        }
    }
}

fn initialize_or_update_viewport<'a>(
    viewports: &'a mut Viewports,
    ids: ViewportIdPair,
    class: ViewportClass,
    mut builder: ViewportBuilder,
    viewport_ui_cb: Option<Arc<dyn Fn(&mut egui::Ui) + Send + Sync>>,
    painter: &mut egui_wgpu::winit::Painter,
) -> &'a mut Viewport {
    use std::collections::btree_map::Entry;

    profiling::function_scope!();

    if builder.icon.is_none() {
        // Inherit icon from parent
        builder.icon = viewports
            .get_mut(&ids.parent)
            .and_then(|vp| vp.builder.icon.clone());
    }

    match viewports.entry(ids.this) {
        Entry::Vacant(entry) => {
            // New viewport:
            log::debug!("Creating new viewport {:?} ({:?})", ids.this, builder.title);
            entry.insert(Viewport {
                ids,
                registration: None,
                parent_registration: None,
                class,
                builder,
                deferred_commands: vec![],
                info: Default::default(),
                actions_requested: Vec::new(),
                viewport_ui_cb,
                window: None,
                surface_generation: 0,
                egui_winit: None,
                pending_raw_input: Default::default(),
            })
        }

        Entry::Occupied(mut entry) => {
            // Patch an existing viewport:
            let viewport = entry.get_mut();

            viewport.class = class;
            viewport.ids.parent = ids.parent;
            viewport.viewport_ui_cb = viewport_ui_cb;

            let (mut delta_commands, recreate) = viewport.builder.patch(builder);

            if recreate {
                log::debug!(
                    "Recreating window for viewport {:?} ({:?})",
                    ids.this,
                    viewport.builder.title
                );
                if let Some(state) = viewport.egui_winit.as_mut() {
                    super::epi_integration::retain_viewport_input_before_state_replacement(
                        viewport.ids.this,
                        &mut viewport.pending_raw_input,
                        state,
                    );
                }
                viewport.window = None;
                viewport.egui_winit = None;
                if let Err(err) = pollster::block_on(painter.set_window(viewport.ids.this, None)) {
                    log::error!(
                        "when rendering viewport_id={:?}, set_window Error {err}",
                        viewport.ids.this
                    );
                }
            }

            viewport.deferred_commands.append(&mut delta_commands);

            entry.into_mut()
        }
    }
}

#[cfg(test)]
mod minimized_full_tests {
    use super::super::frame_output::{
        ClipboardInput, MainOutputTarget, PassDecision, finish_main_viewport_output,
    };
    use super::*;

    #[test]
    fn minimized_full_immediate_real_entry_preserves_child_output_and_paint_ownership() {
        super::super::frame_output::immediate_tests::check_immediate_entry(|backend, viewport| {
            render_immediate_viewport(backend, viewport);
        });
    }

    #[derive(Default)]
    struct FakeWindow {
        input: egui::RawInput,
        clipboard: Option<String>,
        outputs: Vec<egui::PlatformOutput>,
    }

    impl ClipboardInput for FakeWindow {
        fn input(&mut self) -> &mut egui::RawInput {
            &mut self.input
        }
        fn clipboard_text(&mut self) -> Option<String> {
            self.clipboard.clone()
        }
        fn clipboard_image(&mut self) -> Option<egui::ColorImage> {
            None
        }
    }

    impl MainOutputTarget for FakeWindow {
        fn handle_platform_output(&mut self, output: egui::PlatformOutput) {
            for command in &output.commands {
                if let egui::OutputCommand::CopyText(text) = command {
                    self.clipboard = Some(text.clone());
                }
            }
            self.outputs.push(output);
        }
    }

    #[derive(Default)]
    struct FakePainter {
        operations: Vec<(&'static str, egui::TextureId, Option<[usize; 2]>)>,
        submissions: usize,
        surface_calls: Vec<&'static str>,
    }

    impl FakePainter {
        fn apply(&mut self, delta: &mut TexturesDelta) {
            if delta.is_empty() {
                return;
            }
            for (id, updates) in delta.set.drain() {
                for update in updates {
                    self.operations.push(("set", id, update.pos));
                }
            }
            self.submissions += 1;
            for id in delta.free.drain() {
                self.operations.push(("free", id, None));
            }
        }
    }

    #[test]
    fn minimized_full_wgpu_delivers_outputs_resources_and_current_capture_once() {
        let owner = ViewportId::from_hash_of("minimized-main-owner");
        let info = ViewportInfo {
            minimized: Some(true),
            occluded: Some(false),
            ..Default::default()
        };
        let decision = PassDecision::for_viewport(PassMode::Full, &info, false, false, true);
        assert!(decision.run_ui && !decision.paint);
        assert_eq!(info.visible(), Some(false));
        let mut window = FakeWindow {
            input: egui::RawInput {
                viewport_id: owner,
                ..Default::default()
            },
            ..Default::default()
        };
        let sibling = FakeWindow::default();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(80.0, 20.0));
        let platform = egui::PlatformOutput {
            commands: vec![egui::OutputCommand::CopyText("copy\r\ntext".into())],
            ime: Some(egui::output::IMEOutput {
                purpose: egui::IMEPurpose::Normal,
                rect,
                cursor_rect: rect,
                should_interrupt_composition: true,
            }),
            ..Default::default()
        };
        let mut commands = vec![
            egui::ViewportCommand::Title("owner".into()),
            egui::ViewportCommand::RequestCopy,
            egui::ViewportCommand::RequestPaste,
        ];
        let mut actions = vec![ActionRequested::Cut];
        finish_main_viewport_output(platform.clone(), &mut commands, &mut actions, &mut window);
        assert!(window.outputs == vec![platform]);
        assert_eq!(window.input.viewport_id, owner);
        assert_eq!(
            window.input.events,
            vec![
                egui::Event::Cut,
                egui::Event::Copy,
                egui::Event::Paste("copy\ntext".into())
            ]
        );
        assert!(sibling.outputs.is_empty() && sibling.input.events.is_empty());
        assert!(matches!(
            commands.as_slice(),
            [egui::ViewportCommand::Title(_)]
        ));
        assert!(actions.is_empty());

        let id = egui::TextureId::Managed(71);
        let mut delta = TexturesDelta::default();
        delta.push(
            id,
            egui::epaint::ImageDelta::full(
                egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
                egui::TextureOptions::LINEAR,
            ),
        );
        delta.push(
            id,
            egui::epaint::ImageDelta::partial(
                [0, 0],
                egui::ColorImage::filled([1, 1], egui::Color32::BLACK),
                egui::TextureOptions::LINEAR,
            ),
        );
        delta.free(id);
        let mut painter = FakePainter::default();
        // This is the dispatch used by the real root/deferred backend. A
        // non-paint Full may not enter tessellation/present/post_rendering.
        assert_eq!(
            finish_wgpu_main_render(
                decision.paint,
                &mut painter,
                &mut delta,
                |_, delta| {
                    // The headless fixture must end delta ownership before
                    // reporting a forbidden render call, even on failure.
                    delta.clear();
                    panic!("nonpaint entered the surface path");
                },
                FakePainter::apply
            ),
            0.0
        );
        assert!(delta.is_empty());
        assert_eq!(
            painter.operations,
            vec![
                ("set", id, None),
                ("set", id, Some([0, 0])),
                ("free", id, None)
            ]
        );
        assert_eq!(painter.submissions, 1);
        assert!(painter.surface_calls.is_empty());

        // A current-UI capture request must participate in this pass's paint
        // decision and must not remain a command that re-registers it later.
        let captures = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = Arc::clone(&captures);
        let callback = egui::ScreenshotCallback::new(move |_| {
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });
        commands = vec![
            egui::ViewportCommand::Screenshot(callback.clone()),
            egui::ViewportCommand::RequestPaintWhileHidden,
        ];
        window.input.events.clear();
        finish_main_viewport_output(Default::default(), &mut commands, &mut actions, &mut window);
        assert!(commands.is_empty() && window.input.events.is_empty());
        let decision = PassDecision::for_viewport(
            PassMode::Full,
            &info,
            false,
            actions.iter().any(ActionRequested::wants_paint),
            true,
        );
        assert!(decision.paint);
        finish_wgpu_main_render(
            decision.paint,
            &mut painter,
            &mut delta,
            |painter, delta| {
                painter
                    .surface_calls
                    .extend(["tessellate", "paint", "present", "post_rendering"]);
                painter.apply(delta);
                for capture in take_screenshot_requests(&mut actions) {
                    capture.complete(Arc::new(egui::ColorImage::filled(
                        [1, 1],
                        egui::Color32::WHITE,
                    )));
                }
                actions.retain(|action| !matches!(action, ActionRequested::PaintWhileHidden));
                0.0
            },
            FakePainter::apply,
        );
        callback.complete(Arc::new(egui::ColorImage::filled(
            [1, 1],
            egui::Color32::WHITE,
        )));
        assert_eq!(captures.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(actions.is_empty());
        assert_eq!(
            painter.submissions, 1,
            "no duplicate empty resource submission"
        );
        finish_main_viewport_output(Default::default(), &mut commands, &mut actions, &mut window);
        assert!(commands.is_empty() && actions.is_empty() && window.input.events.is_empty());
    }
}

#[cfg(test)]
mod stage2c1_tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn viewport_input_follows_logical_lifetime_and_gc_preserves_sibling() {
        struct HeadlessDisplay;
        impl raw_window_handle::HasDisplayHandle for HeadlessDisplay {
            fn display_handle(
                &self,
            ) -> std::result::Result<
                raw_window_handle::DisplayHandle<'_>,
                raw_window_handle::HandleError,
            > {
                Err(raw_window_handle::HandleError::Unavailable)
            }
        }
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let mut painter = pollster::block_on(egui_wgpu::winit::Painter::new(
            ctx.clone(),
            Default::default(),
            false,
            Default::default(),
        ));
        let mut viewports = Viewports::default();
        let mut window_ids = HashMap::default();
        let child = ViewportId::from_hash_of("input-child");
        let sibling = ViewportId::from_hash_of("input-sibling");
        for (id, text) in [(child, "old-child"), (sibling, "old-sibling")] {
            let viewport = initialize_or_update_viewport(
                &mut viewports,
                ViewportIdPair::from_self_and_parent(id, ViewportId::ROOT),
                ViewportClass::Deferred,
                ViewportBuilder::default(),
                Some(Arc::new(|_| {})),
                &mut painter,
            );
            viewport.pending_raw_input = egui::RawInput {
                viewport_id: id,
                events: vec![egui::Event::Text(text.into())],
                ..Default::default()
            };
        }
        let queued = vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::WindowFocused(false),
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            egui::Event::Ime(egui::ImeEvent::Commit("terminal".into())),
        ];
        let mut state =
            egui_winit::State::new(ctx.clone(), child, &HeadlessDisplay, None, None, None);
        state.egui_input_mut().events = queued.clone();
        viewports.get_mut(&child).unwrap().egui_winit = Some(state);
        // Native resource recreation and deferred/immediate class changes do
        // not end the logical owner. Its pending input stays with it.
        let viewport = initialize_or_update_viewport(
            &mut viewports,
            ViewportIdPair::from_self_and_parent(child, ViewportId::ROOT),
            ViewportClass::Immediate,
            ViewportBuilder::default().with_active(false),
            None,
            &mut painter,
        );
        let input = super::super::epi_integration::take_viewport_input(
            &mut viewport.pending_raw_input,
            || egui::RawInput {
                viewport_id: child,
                events: vec![egui::Event::Text("new-child".into())],
                ..Default::default()
            },
        );
        assert_eq!(input.viewport_id, child);
        let mut expected = vec![egui::Event::Text("old-child".into())];
        expected.extend(queued);
        expected.push(egui::Event::Text("new-child".into()));
        assert_eq!(input.events, expected);
        viewport.pending_raw_input = input;

        // Normal output GC ends the owner without a real window or a UI pass
        // between removal and creation of a fresh owner with the same ID.
        let output = ctx.run_ui(Default::default(), |ui| {
            ui.ctx()
                .show_viewport_deferred(sibling, ViewportBuilder::default(), |_, _| {});
        });
        remove_viewports_not_in(
            &mut viewports,
            &mut painter,
            &mut window_ids,
            &output.viewport_output,
        );
        output.drop_without_applying_deltas();
        assert!(!viewports.contains_key(&child));
        let recreated = initialize_or_update_viewport(
            &mut viewports,
            ViewportIdPair::from_self_and_parent(child, ViewportId::ROOT),
            ViewportClass::Deferred,
            ViewportBuilder::default(),
            Some(Arc::new(|_| {})),
            &mut painter,
        );
        assert!(recreated.pending_raw_input.events.is_empty());
        assert!(recreated.egui_winit.is_none());
        assert_eq!(
            viewports[&sibling].pending_raw_input.events,
            vec![egui::Event::Text("old-sibling".into())]
        );
    }

    #[test]
    fn immediate_screenshot_callbacks_keep_identity_order_and_one_shot_completion() {
        let (sender, receiver) = mpsc::channel();
        let first_sender = sender.clone();
        let first = egui::ScreenshotCallback::new(move |image| {
            first_sender.send((17, image.size)).unwrap();
        });
        let second = egui::ScreenshotCallback::new(move |image| {
            sender.send((29, image.size)).unwrap();
        });
        let mut child_actions = vec![
            ActionRequested::Copy,
            ActionRequested::Screenshot(first.clone()),
            ActionRequested::PaintWhileHidden,
            ActionRequested::Screenshot(second.clone()),
            ActionRequested::Paste,
        ];
        let root = egui::ScreenshotCallback::new(|_| panic!("child completed root capture"));
        let mut root_actions = vec![ActionRequested::Screenshot(root.clone())];
        let captures = take_screenshot_requests(&mut child_actions);
        assert_eq!(captures, vec![first.clone(), second]);
        assert!(matches!(
            child_actions.as_slice(),
            [
                ActionRequested::Copy,
                ActionRequested::PaintWhileHidden,
                ActionRequested::Paste
            ]
        ));
        assert!(take_screenshot_requests(&mut child_actions).is_empty());
        assert_eq!(take_screenshot_requests(&mut root_actions), vec![root]);
        let image = Arc::new(egui::ColorImage::filled([3, 2], egui::Color32::WHITE));
        for capture in captures {
            capture.complete(Arc::clone(&image));
        }
        first.complete(image);
        assert_eq!(
            receiver.try_iter().collect::<Vec<_>>(),
            vec![(17, [3, 2]), (29, [3, 2])]
        );
    }

    #[test]
    fn disappearing_viewport_keeps_unapplied_delta_in_shared_renderer_owner() {
        let ctx = egui::Context::default();
        let painter = pollster::block_on(egui_wgpu::winit::Painter::new(
            ctx.clone(),
            Default::default(),
            false,
            Default::default(),
        ));
        let child_id = ViewportId::from_hash_of("stage2c1-child");
        let mut shared = SharedState {
            egui_ctx: ctx.clone(),
            viewports: Default::default(),
            painter,
            pending_deltas: Default::default(),
            viewport_from_window: Default::default(),
            focused_viewport: Some(ViewportId::ROOT),
            resized_viewport: None,
            clipboard_shortcuts: true,
        };
        for ids in [
            ViewportIdPair::ROOT,
            ViewportIdPair::from_self_and_parent(child_id, ViewportId::ROOT),
        ] {
            initialize_or_update_viewport(
                &mut shared.viewports,
                ids,
                ViewportClass::Immediate,
                ViewportBuilder::default(),
                None,
                &mut shared.painter,
            );
        }
        shared
            .viewports
            .get_mut(&ViewportId::ROOT)
            .unwrap()
            .surface_generation = 17;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            ui.label("delivery fixture");
        });
        shared
            .pending_deltas
            .append(core::mem::take(&mut output.textures_delta));
        let active = core::mem::take(&mut output.viewport_output);
        output.drop_without_applying_deltas(); // headless output; delta authority moved above
        let before = texture_delta_stats(&shared.pending_deltas);
        assert!(before.0 > 0);
        assert!(shared.painter.render_state().is_none());
        shared
            .painter
            .apply_textures_delta(&mut shared.pending_deltas);
        assert_eq!(texture_delta_stats(&shared.pending_deltas), before);
        remove_viewports_not_in(
            &mut shared.viewports,
            &mut shared.painter,
            &mut shared.viewport_from_window,
            &active,
        );
        assert!(!shared.viewports.contains_key(&child_id));
        assert_eq!(shared.viewports[&ViewportId::ROOT].surface_generation, 17);
        assert_eq!(shared.focused_viewport, Some(ViewportId::ROOT));
        assert_eq!(texture_delta_stats(&shared.pending_deltas), before);
        // SharedState termination explicitly ends delivery; viewport removal did not.
    }

    #[test]
    fn immediate_delta_diagnostics_count_ordered_updates_and_rgba_bytes() {
        let mut delta = TexturesDelta::default();
        let id = egui::TextureId::Managed(17);
        delta.push(
            id,
            egui::epaint::ImageDelta::full(
                egui::ColorImage::filled([4, 4], egui::Color32::BLACK),
                egui::TextureOptions::LINEAR,
            ),
        );
        delta.push(
            id,
            egui::epaint::ImageDelta::partial(
                [1, 2],
                egui::ColorImage::filled([2, 1], egui::Color32::WHITE),
                egui::TextureOptions::LINEAR,
            ),
        );
        delta.free(egui::TextureId::Managed(29));
        assert_eq!(texture_delta_stats(&delta), (2, 1, 72));
        // Inspection has no delivery authority: both ordered updates remain available.
        assert_eq!(delta.set[&id][0].pos, None);
        assert_eq!(delta.set[&id][1].pos, Some([1, 2]));
        delta.clear(); // headless diagnostic fixture intentionally has no renderer destination
    }
}

#[cfg(test)]
mod stage2c4a_tests {
    use super::super::viewport_lifecycle::{ChildCloseBackend as _, dispatch_child_close_requests};
    use super::*;

    struct HeadlessDisplay;

    impl raw_window_handle::HasDisplayHandle for HeadlessDisplay {
        fn display_handle(
            &self,
        ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError>
        {
            Err(raw_window_handle::HandleError::Unavailable)
        }
    }

    fn fixture(ctx: &egui::Context, child: ViewportId, sibling: ViewportId) -> SharedState {
        let painter = pollster::block_on(egui_wgpu::winit::Painter::new(
            ctx.clone(),
            Default::default(),
            false,
            Default::default(),
        ));
        let mut shared = SharedState {
            egui_ctx: ctx.clone(),
            viewports: Default::default(),
            painter,
            pending_deltas: Default::default(),
            viewport_from_window: Default::default(),
            focused_viewport: Some(child),
            resized_viewport: Some(child),
            clipboard_shortcuts: true,
        };
        for id in [ViewportId::ROOT, child, sibling] {
            initialize_or_update_viewport(
                &mut shared.viewports,
                ViewportIdPair::from_self_and_parent(id, ViewportId::ROOT),
                ViewportClass::Deferred,
                ViewportBuilder::default(),
                None,
                &mut shared.painter,
            );
        }
        shared
    }

    #[test]
    fn child_close_wgpu_cancel_consumes_snapshots_and_fresh_close_retires_without_ui() {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let child = ViewportId::from_hash_of("close-wgpu-child");
        let sibling = ViewportId::from_hash_of("close-wgpu-sibling");
        let child_callbacks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let callback_counter = child_callbacks.clone();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let callback_counter = callback_counter.clone();
            ui.ctx()
                .show_viewport_deferred(child, ViewportBuilder::default(), move |ui, _| {
                    callback_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    assert!(!ui.input(|input| input.viewport().close_requested()));
                });
            ui.ctx()
                .show_viewport_deferred(sibling, ViewportBuilder::default(), |_, _| {});
        });
        let child_ui = output.viewport_output[&child]
            .viewport_ui_cb
            .clone()
            .unwrap();
        let mut shared = fixture(&ctx, child, sibling);
        shared
            .pending_deltas
            .append(core::mem::take(&mut output.textures_delta));
        output.drop_without_applying_deltas();
        let ordinary = vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Text("keep".into()),
            egui::Event::WindowFocused(false),
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            egui::Event::Ime(egui::ImeEvent::Commit("terminal".into())),
        ];
        // OS ingress, including a skipped/minimized child and snapshots retained
        // by other logical owners. The ordinary input sequence is untouched.
        shared.viewports.get_mut(&child).unwrap().info.minimized = Some(true);
        shared
            .viewports
            .get_mut(&child)
            .unwrap()
            .info
            .events
            .push(egui::ViewportEvent::Close);
        for (id, viewport) in &mut shared.viewports {
            viewport.pending_raw_input.events = ordinary.clone();
            viewport
                .pending_raw_input
                .viewports
                .entry(child)
                .or_default()
                .events
                .push(egui::ViewportEvent::Close);
            let mut state =
                egui_winit::State::new(ctx.clone(), *id, &HeadlessDisplay, None, None, None);
            state.egui_input_mut().events = ordinary.clone();
            state
                .egui_input_mut()
                .viewports
                .entry(child)
                .or_default()
                .events
                .push(egui::ViewportEvent::Close);
            viewport.egui_winit = Some(state);
        }
        let delta_before = texture_delta_stats(&shared.pending_deltas);
        let mut shared = Rc::new(RefCell::new(shared));
        struct Owner {
            shared: Rc<RefCell<SharedState>>,
            calls: Vec<ViewportId>,
            response: crate::ViewportCloseResponse,
        }
        impl App for Owner {
            fn ui(&mut self, _: &mut egui::Ui, _: &mut crate::Frame) {
                panic!("lifecycle must not run application UI");
            }
            fn on_viewport_close_requested(
                &mut self,
                _: &egui::Context,
                id: ViewportId,
            ) -> crate::ViewportCloseResponse {
                // The production dispatcher must release the backend borrow.
                let _exclusive_backend_access = self.shared.borrow_mut();
                self.calls.push(id);
                self.response
            }
        }
        let mut owner = Owner {
            shared: shared.clone(),
            calls: vec![],
            response: crate::ViewportCloseResponse::Cancel,
        };
        assert_eq!(child_callbacks.load(std::sync::atomic::Ordering::SeqCst), 0);
        dispatch_child_close_requests(&ctx, &mut owner, &mut shared);
        dispatch_child_close_requests(&ctx, &mut owner, &mut shared);
        assert_eq!(owner.calls, [child]);
        assert_eq!(child_callbacks.load(std::sync::atomic::Ordering::SeqCst), 0);
        {
            let shared = shared.borrow();
            assert!(!shared.viewports[&child].info.close_requested());
            for viewport in shared.viewports.values() {
                assert_eq!(viewport.pending_raw_input.events, ordinary);
                assert!(!viewport.pending_raw_input.viewports[&child].close_requested());
                let input = viewport.egui_winit.as_ref().unwrap().egui_input();
                assert_eq!(input.events, ordinary);
                assert!(!input.viewports[&child].close_requested());
            }
        }
        // The modal ends and the child restores. First advance root logic only:
        // this is a real non-UI Context operation and must not redeliver Close.
        shared
            .borrow_mut()
            .viewports
            .get_mut(&child)
            .unwrap()
            .info
            .minimized = Some(false);
        let metadata: egui::ViewportIdMap<ViewportInfo> = shared
            .borrow()
            .viewports
            .iter()
            .map(|(id, viewport)| (*id, viewport.info.clone()))
            .collect();
        let logic_output = ctx.run_logic(
            &egui::RawInput {
                viewports: metadata.clone(),
                ..Default::default()
            },
            |ctx| {
                assert!(!ctx.input(|input| input.raw.viewports[&child].close_requested()));
            },
        );
        assert!(logic_output.viewport_commands.is_empty());
        dispatch_child_close_requests(&ctx, &mut owner, &mut shared);
        assert_eq!(owner.calls, [child]);
        assert_eq!(child_callbacks.load(std::sync::atomic::Ordering::SeqCst), 0);

        // Then run the actual retained callback through a child Full. Use the
        // same pending-plus-State input owner as the native preparation path.
        let mut input = {
            let mut shared = shared.borrow_mut();
            let viewport = shared.viewports.get_mut(&child).unwrap();
            let state = viewport.egui_winit.as_mut().unwrap();
            super::super::epi_integration::take_viewport_input(
                &mut viewport.pending_raw_input,
                || std::mem::take(state.egui_input_mut()),
            )
        };
        input.viewport_id = child;
        input.viewports = metadata.clone();
        let expected_input = ordinary
            .iter()
            .chain(&ordinary)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(input.events, expected_input);
        let output = ctx.run_ui(input, |ui| {
            assert_eq!(ui.input(|input| input.raw.events.clone()), expected_input);
            child_ui(ui);
        });
        output.drop_without_applying_deltas();
        dispatch_child_close_requests(&ctx, &mut owner, &mut shared);
        assert_eq!(child_callbacks.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(owner.calls, [child]);

        // A parent Full keeps the child declared and still sees no rejected
        // Close. Neither actual Full may turn modal completion into a retry.
        let callback = child_ui.clone();
        let output = ctx.run_ui(
            egui::RawInput {
                viewports: metadata,
                ..Default::default()
            },
            |ui| {
                assert!(!ui.input(|input| input.raw.viewports[&child].close_requested()));
                let callback = callback.clone();
                ui.ctx()
                    .show_viewport_deferred(child, ViewportBuilder::default(), move |ui, _| {
                        callback(ui)
                    });
                ui.ctx()
                    .show_viewport_deferred(sibling, ViewportBuilder::default(), |_, _| {});
            },
        );
        assert!(output.viewport_output.contains_key(&child));
        output.drop_without_applying_deltas();
        dispatch_child_close_requests(&ctx, &mut owner, &mut shared);
        assert_eq!(owner.calls, [child]);
        assert_eq!(child_callbacks.load(std::sync::atomic::Ordering::SeqCst), 1);

        // A fresh programmatic Close uses the same hook and may now be accepted.
        // No native window exists, so this still verifies pre-window delivery.
        shared
            .borrow_mut()
            .viewports
            .get_mut(&child)
            .unwrap()
            .process_commands(
                &ctx,
                vec![
                    egui::ViewportCommand::Close,
                    egui::ViewportCommand::Title("kept".into()),
                ],
            );
        owner.response = crate::ViewportCloseResponse::Close;
        dispatch_child_close_requests(&ctx, &mut owner, &mut shared);
        dispatch_child_close_requests(&ctx, &mut owner, &mut shared);
        assert_eq!(owner.calls, [child, child]);
        {
            let shared = shared.borrow();
            assert!(!shared.viewports.contains_key(&child));
            assert!(shared.viewports.contains_key(&sibling));
            assert_eq!(
                shared.viewports[&sibling].pending_raw_input.events,
                ordinary
            );
            assert_eq!(texture_delta_stats(&shared.pending_deltas), delta_before);
            assert_eq!(shared.focused_viewport, None);
            assert_eq!(shared.resized_viewport, None);
        }
        let output = ctx.run_ui(
            egui::RawInput {
                viewport_id: sibling,
                viewports: [
                    (ViewportId::ROOT, ViewportInfo::default()),
                    (sibling, ViewportInfo::default()),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            },
            |_| {},
        );
        assert!(!output.viewport_output.contains_key(&child));
        output.drop_without_applying_deltas();
        let output = ctx.run_ui(Default::default(), |_| {});
        assert!(!output.viewport_output.contains_key(&child));
        output.drop_without_applying_deltas();
    }

    #[test]
    fn child_close_wgpu_default_delegates_and_root_stays_on_existing_path() {
        struct DefaultOwner;
        impl App for DefaultOwner {
            fn ui(&mut self, _: &mut egui::Ui, _: &mut crate::Frame) {
                panic!("delegation must not force a UI pass");
            }
        }
        let ctx = egui::Context::default();
        let child = ViewportId::from_hash_of("close-wgpu-default-child");
        let sibling = ViewportId::from_hash_of("close-wgpu-default-sibling");
        let mut shared = fixture(&ctx, child, sibling);
        for id in [ViewportId::ROOT, child] {
            shared
                .viewports
                .get_mut(&id)
                .unwrap()
                .info
                .events
                .push(egui::ViewportEvent::Close);
        }
        let mut shared = Rc::new(RefCell::new(shared));
        let before = shared.close_requests();
        dispatch_child_close_requests(&ctx, &mut DefaultOwner, &mut shared);
        assert_eq!(shared.close_requests(), before);
        assert!(shared.borrow().viewports[&child].info.close_requested());
        assert!(
            shared.borrow().viewports[&ViewportId::ROOT]
                .info
                .close_requested()
        );
        assert!(shared.borrow().viewports.contains_key(&sibling));
    }

    #[test]
    fn child_close_wgpu_subtree_and_usual_gc_share_fake_native_resource_cleanup() {
        struct Resource(std::rc::Rc<std::cell::Cell<usize>>);
        impl Drop for Resource {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let ctx = egui::Context::default();
        let child = ViewportId::from_hash_of("close-wgpu-subtree-child");
        let grandchild = ViewportId::from_hash_of("close-wgpu-subtree-grandchild");
        let sibling = ViewportId::from_hash_of("close-wgpu-subtree-sibling");
        let mut shared = fixture(&ctx, child, sibling);
        initialize_or_update_viewport(
            &mut shared.viewports,
            ViewportIdPair::from_self_and_parent(grandchild, child),
            ViewportClass::Immediate,
            ViewportBuilder::default(),
            None,
            &mut shared.painter,
        );
        let retired = native_viewport_subtree(&shared.viewports, child);
        assert_eq!(retired.len(), 2);
        assert!(retired.contains(&grandchild));
        let active: ViewportIdSet = shared
            .viewports
            .keys()
            .copied()
            .filter(|id| !retired.contains(id))
            .collect();
        let drops = Rc::new(std::cell::Cell::new(0));
        // Fake native window, State and surface owners: no OS or GPU creation.
        let mut resources: HashMap<_, _> = [ViewportId::ROOT, child, grandchild, sibling]
            .into_iter()
            .map(|id| {
                (
                    id,
                    [
                        Resource(drops.clone()),
                        Resource(drops.clone()),
                        Resource(drops.clone()),
                    ],
                )
            })
            .collect();
        let mut mapping: HashMap<_, _> = [
            (0, ViewportId::ROOT),
            (1, child),
            (2, grandchild),
            (3, sibling),
        ]
        .into_iter()
        .collect();
        retain_viewport_resources(&mut shared.viewports, &mut mapping, &active, |active| {
            resources.retain(|id, _| active.contains(id));
        });
        assert_eq!(drops.get(), 6);
        assert_eq!(mapping.len(), 2);
        assert_eq!(resources.len(), 2);
        assert!(!shared.viewports.contains_key(&child));
        assert!(!shared.viewports.contains_key(&grandchild));
        assert!(shared.viewports.contains_key(&sibling));
        // Repeating ordinary GC does not retire retained siblings a second time.
        retain_viewport_resources(&mut shared.viewports, &mut mapping, &active, |active| {
            resources.retain(|id, _| active.contains(id));
        });
        assert_eq!(drops.get(), 6);
    }
}

#[cfg(test)]
mod viewport_retirement_tests {
    use super::super::viewport_lifecycle::viewport_retirement_tests::{self as scenarios, Adapter};
    use super::*;
    type Headless = Rc<RefCell<SharedState>>;

    impl Adapter for Headless {
        fn new(ctx: &egui::Context) -> Self {
            Rc::new(RefCell::new(SharedState {
                egui_ctx: ctx.clone(),
                viewports: Default::default(),
                painter: pollster::block_on(egui_wgpu::winit::Painter::new(
                    ctx.clone(),
                    Default::default(),
                    false,
                    Default::default(),
                )),
                pending_deltas: Default::default(),
                viewport_from_window: Default::default(),
                focused_viewport: None,
                resized_viewport: None,
                clipboard_shortcuts: true,
            }))
        }
        fn sync(&mut self, ctx: &egui::Context, outputs: &OrderedViewportIdMap<ViewportOutput>) {
            let mut state = self.borrow_mut();
            let SharedState {
                viewports,
                painter,
                viewport_from_window,
                ..
            } = &mut *state;
            handle_viewport_output(ctx, outputs, viewports, painter, viewport_from_window);
            for (id, viewport) in viewports {
                viewport.pending_raw_input.viewport_id = *id;
                if *id != ViewportId::ROOT {
                    viewport
                        .pending_raw_input
                        .viewports
                        .entry(*id)
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
            self.borrow()
                .viewports
                .get(&id)
                .and_then(|vp| vp.registration)
        }
        fn set_close(&mut self, id: ViewportId) {
            self.borrow_mut()
                .viewports
                .get_mut(&id)
                .unwrap()
                .info
                .events
                .push(egui::ViewportEvent::Close);
        }
        fn pending(&self, id: ViewportId) -> Vec<egui::Event> {
            self.borrow().viewports[&id]
                .pending_raw_input
                .events
                .clone()
        }
        fn accept_delta(&mut self, delta: egui::TexturesDelta) {
            super::super::viewport_lifecycle::append_texture_delta(
                &mut self.borrow_mut().pending_deltas,
                delta,
            );
        }
        fn take_delta(&mut self) -> egui::TexturesDelta {
            core::mem::take(&mut self.borrow_mut().pending_deltas)
        }
        fn replace_stale_native(&mut self, ctx: &egui::Context, id: ViewportId) {
            replace_stale_wgpu_viewport(self, ctx, id);
        }
        fn seed_stale_native(&mut self, ctx: &egui::Context, id: ViewportId) -> egui::TextureId {
            let texture = scenarios::stale_texture(ctx);
            let texture_id = texture.id();
            let mut state = self.borrow_mut();
            let viewport = state.viewports.get_mut(&id).unwrap();
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
            let state = self.borrow();
            let viewport = &state.viewports[&id];
            let mut events = viewport.pending_raw_input.events.clone();
            if let Some(input) = &viewport.egui_winit {
                events.extend(input.egui_input().events.clone());
            }
            events
        }
    }

    #[test]
    fn viewport_retirement_wgpu_full_logic_gc_discard() {
        scenarios::full_logic_gc_discard::<Headless>();
    }
    #[test]
    fn viewport_retirement_wgpu_logic_only() {
        scenarios::logic_only_and_pass_outside_hook::<Headless>();
    }
    #[test]
    fn viewport_retirement_wgpu_close_request_separation() {
        scenarios::close_request_separation::<Headless>();
    }
    #[test]
    fn viewport_retirement_wgpu_context_gc_native_descendants_ime() {
        scenarios::context_gc_native_descendants_and_ime::<Headless>();
    }
    #[test]
    fn viewport_retirement_wgpu_old_output_reopen_duplicate() {
        scenarios::old_output_reopen_and_duplicate::<Headless>();
    }
    #[test]
    fn viewport_retirement_wgpu_overlapping_gc_root_close() {
        scenarios::overlapping_gc_and_root_close::<Headless>();
    }
    #[test]
    fn viewport_retirement_wgpu_delta_callback_drop() {
        scenarios::delta_and_callback_drop::<Headless>();
    }
    #[test]
    fn viewport_retirement_wgpu_immediate_nested() {
        scenarios::immediate_nested::<Headless>();
    }
    #[test]
    fn viewport_retirement_wgpu_hook_captures_lifetime_before_reopen() {
        scenarios::legacy_hook_captures_lifetime_before_reopen::<Headless>();
    }
    #[test]
    fn viewport_retirement_wgpu_discard_immediate_reopen_replaces_native() {
        scenarios::discard_immediate_reopen_replaces_native::<Headless>();
    }
    #[test]
    fn discard_texture_delivery_wgpu_native_callback_drop_gc_reopen_ime() {
        scenarios::discard_texture_delivery_native_reopen::<Headless>();
    }

    #[test]
    fn discard_texture_delivery_wgpu_production_scope_weak_pending_and_restore() {
        let ctx = egui::Context::default();
        let shared = <Headless as Adapter>::new(&ctx);
        let scope = enter_discard_texture_delivery(&ctx, &shared);
        let mut handle = Some(ctx.load_texture(
            "pending",
            egui::ColorImage::filled([2, 2], egui::Color32::BLACK),
            egui::TextureOptions::NEAREST,
        ));
        handle.as_mut().unwrap().set_partial(
            [1, 0],
            egui::ColorImage::filled([1, 1], egui::Color32::RED),
            egui::TextureOptions::NEAREST,
        );
        let id = handle.as_ref().unwrap().id();
        let mut pass = 0;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            if pass == 0 {
                ui.ctx().request_discard("production sink pending");
            } else {
                drop(handle.take());
            }
            pass += 1;
        });
        // No renderer exists: the production sink transfers to the existing
        // pending owner. Later free acceptance must not cancel whole/partial.
        assert!(shared.borrow().painter.render_state().is_none());
        super::super::viewport_lifecycle::append_texture_delta(
            &mut shared.borrow_mut().pending_deltas,
            core::mem::take(&mut output.textures_delta),
        );
        assert_eq!(shared.borrow().pending_deltas.set[&id].len(), 2);
        assert!(shared.borrow().pending_deltas.free.contains(&id));
        output.drop_without_applying_deltas();
        drop(scope);
        assert!(!ctx.has_discard_texture_delivery());
        // The scope captures Weak; it does not extend the backend lifetime.
        let scope = enter_discard_texture_delivery(&ctx, &shared);
        let weak = Rc::downgrade(&shared);
        drop(shared);
        assert!(weak.upgrade().is_none());
        drop(scope);
    }
}
