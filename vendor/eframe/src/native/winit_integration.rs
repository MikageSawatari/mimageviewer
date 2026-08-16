use std::{sync::Arc, time::Instant};

use winit::{
    dpi::PhysicalSize,
    event_loop::ActiveEventLoop,
    window::{Window, WindowId},
};

use egui::ViewportId;
#[cfg(feature = "accesskit")]
use egui_winit::accesskit_winit;

/// Returns `true` if the window is invisible or minimized.
///
/// These windows don't receive `RedrawRequested` events on Windows,
/// so they need special handling to keep processing viewport commands.
pub fn is_invisible_or_minimized(window: &Window) -> bool {
    window.is_visible() == Some(false) || window.is_minimized() == Some(true)
}

/// Create an egui context, restoring it from storage if possible.
pub fn create_egui_context(storage: Option<&dyn crate::Storage>) -> egui::Context {
    profiling::function_scope!();

    pub const IS_DESKTOP: bool = cfg!(any(
        target_os = "freebsd",
        target_os = "linux",
        target_os = "macos",
        target_os = "openbsd",
        target_os = "windows",
    ));

    let egui_ctx = egui::Context::default();

    egui_ctx.set_embed_viewports(!IS_DESKTOP);

    egui_ctx.options_mut(|o| {
        // eframe supports multi-pass (Context::request_discard).
        o.max_passes = 2.try_into().unwrap();
    });

    let memory = crate::native::epi_integration::load_egui_memory(storage).unwrap_or_default();
    egui_ctx.memory_mut(|mem| *mem = memory);

    egui_ctx
}

/// The custom even `eframe` uses with the [`winit`] event loop.
#[derive(Debug)]
pub enum UserEvent {
    /// A repaint is requested.
    RequestRepaint {
        /// What to repaint.
        viewport_id: ViewportId,

        /// When to repaint.
        when: Instant,

        /// What the cumulative pass number was when the repaint was _requested_.
        cumulative_pass_nr: u64,
    },

    /// A request related to [`accesskit`](https://accesskit.dev/).
    #[cfg(feature = "accesskit")]
    AccessKitActionRequest(accesskit_winit::Event),
}

#[cfg(feature = "accesskit")]
impl From<accesskit_winit::Event> for UserEvent {
    fn from(inner: accesskit_winit::Event) -> Self {
        Self::AccessKitActionRequest(inner)
    }
}

pub trait WinitApp {
    fn egui_ctx(&self) -> Option<&egui::Context>;

    fn window(&self, window_id: WindowId) -> Option<Arc<Window>>;

    fn window_id_from_viewport_id(&self, id: ViewportId) -> Option<WindowId>;

    /// The render target currently owned by window_id.
    ///
    /// Backends with explicit surface lifecycle tracking should override this.
    /// The fallback still gives the scheduler a per-window viewport identity and
    /// current client size, which keeps legacy backends source-compatible.
    fn render_target(&self, window_id: WindowId) -> Option<RenderTarget> {
        let window = self.window(window_id)?;
        Some(RenderTarget {
            window_id,
            viewport_id: ViewportId::from_hash_of(window_id),
            surface_generation: 0,
            size: window.inner_size(),
        })
    }

    fn save(&mut self);

    fn save_and_destroy(&mut self);

    fn run_ui_and_paint(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
    ) -> crate::Result<EventResult>;

    fn suspended(&mut self, event_loop: &ActiveEventLoop) -> crate::Result<EventResult>;

    fn resumed(&mut self, event_loop: &ActiveEventLoop) -> crate::Result<EventResult>;

    fn device_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        device_id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) -> crate::Result<EventResult>;

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: winit::event::WindowEvent,
    ) -> crate::Result<EventResult>;

    #[cfg(feature = "accesskit")]
    fn on_accesskit_event(&mut self, event: accesskit_winit::Event) -> crate::Result<EventResult>;
}

/// Identity of the surface that a scheduled frame is allowed to render into.
///
/// Keeping this metadata beside each dirty window prevents a resize or viewport
/// recreation in one context from publishing stale work into another context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderTarget {
    pub window_id: WindowId,
    pub viewport_id: ViewportId,
    pub surface_generation: u64,
    pub size: PhysicalSize<u32>,
}

/// Why a producer asked for a frame without normal event-loop coalescing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepaintNowReason {
    Bootstrap,
    AccessKit,
    InteractiveResize,
}

/// A typed immediate repaint request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepaintNowRequest {
    pub window_id: WindowId,
    pub reason: RepaintNowReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventResult {
    Wait,

    /// Compatibility result for backends that have not attached provenance yet.
    ///
    /// The event-owning wrapper must convert known producers to
    /// `RepaintNowWithReason` before scheduling or painting.
    #[allow(dead_code)]
    // Constructed by the legacy glow backend, which may be feature-disabled.
    RepaintNow(WindowId),

    /// A repaint request whose event provenance has been preserved.
    ///
    /// Only `InteractiveResize` may paint during message dispatch. Bootstrap
    /// and AccessKit requests are drained by the outer render phase.
    RepaintNowWithReason(RepaintNowRequest),

    /// Queues a repaint for once the event loop handles its next redraw. Exists
    /// so that multiple input events can be handled in one frame. Does not
    /// cause any delay like `RepaintNow`.
    RepaintNext(WindowId),

    RepaintAt(WindowId, Instant),

    /// Causes a save of the client state when the persistence feature is enabled.
    Save,

    /// Starts the process of ending eframe execution whilst allowing for proper
    /// clean up of resources.
    ///
    /// # Warning
    /// This event **must** occur before [`Exit`] to correctly exit eframe code.
    /// If in doubt, return this event.
    ///
    /// [`Exit`]: [EventResult::Exit]
    CloseRequested,

    /// The event loop will exit, now.
    /// The correct circumstance to return this event is in response to a winit "Destroyed" event.
    ///
    /// # Warning
    /// The [`CloseRequested`] **must** occur before this event to ensure that winit
    /// is able to remove any open windows. Otherwise the window(s) will remain open
    /// until the program terminates.
    ///
    /// [`CloseRequested`]: EventResult::CloseRequested
    Exit,
}

impl EventResult {
    pub fn repaint_now(window_id: WindowId, reason: RepaintNowReason) -> Self {
        Self::RepaintNowWithReason(RepaintNowRequest { window_id, reason })
    }

    /// Attach provenance to a legacy backend's RepaintNow result.
    ///
    /// The wrapper calls this only while it still owns the originating event;
    /// no timing, geometry comparison, focus, or detached predicate is used.
    pub fn with_repaint_now_reason(self, reason: RepaintNowReason) -> Self {
        match self {
            Self::RepaintNow(window_id) => Self::repaint_now(window_id, reason),
            _ => self,
        }
    }
}

#[cfg(feature = "accesskit")]
pub(crate) fn on_accesskit_window_event(
    egui_winit: &mut egui_winit::State,
    window_id: WindowId,
    event: &accesskit_winit::WindowEvent,
) -> EventResult {
    match event {
        accesskit_winit::WindowEvent::InitialTreeRequested => {
            egui_winit.egui_ctx().enable_accesskit();
            // Because we can't provide the initial tree synchronously
            // (because that would require the activation handler to access
            // the same mutable state as the winit event handler), some
            // AccessKit platform adapters will use a placeholder tree
            // until we send the first tree update. To minimize the possible
            // bad effects of that workaround, repaint and send the tree
            // immediately.
            EventResult::repaint_now(window_id, RepaintNowReason::AccessKit)
        }
        accesskit_winit::WindowEvent::ActionRequested(request) => {
            egui_winit.on_accesskit_action_request(request.clone());
            // As a form of user input, accessibility actions should cause
            // a repaint, but not until the next regular frame.
            EventResult::RepaintNext(window_id)
        }
        accesskit_winit::WindowEvent::AccessibilityDeactivated => {
            egui_winit.egui_ctx().disable_accesskit();
            // Disabling AccessKit support should have no visible effect,
            // so there's no need to repaint.
            EventResult::Wait
        }
    }
}
