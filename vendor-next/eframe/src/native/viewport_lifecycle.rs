//! Non-drawing child Close acceptance, separate from input/UI and root shutdown.

use egui::{Context, RawInput, ViewportId};

use crate::{App, ViewportCloseResponse};

/// Native adapters borrow their resource maps only within each method. The
/// application hook must be free to inspect Context and enter its own owner.
pub(super) trait ChildCloseBackend {
    fn close_requests(&self) -> Vec<ViewportId>;
    fn consume_close(&mut self, id: ViewportId);
    fn retire(&mut self, ctx: &Context, id: ViewportId);
}

/// Called at native event completion or before/after a complete outer UI/logic
/// operation, never from immediate callbacks, plugin hooks or inside a UI pass.
pub(super) fn dispatch_child_close_requests(
    ctx: &Context,
    app: &mut dyn App,
    backend: &mut impl ChildCloseBackend,
) {
    for id in backend.close_requests() {
        if id == ViewportId::ROOT {
            continue;
        }
        // A preceding accepted ancestor may already have removed this owner.
        if !backend.close_requests().contains(&id) {
            continue;
        }
        match app.on_viewport_close_requested(ctx, id) {
            ViewportCloseResponse::DelegateToUi => {}
            response => {
                backend.consume_close(id);
                ctx.consume_viewport_close(id);
                if matches!(response, ViewportCloseResponse::Close) {
                    backend.retire(ctx, id);
                }
            }
        }
    }
}

/// Remove only this lifecycle request from a pending metadata snapshot. Key,
/// pointer, focus, modifiers and IME events retain their order and owner.
pub(super) fn consume_close_in_input(input: &mut RawInput, id: ViewportId) {
    if let Some(info) = input.viewports.get_mut(&id) {
        info.events
            .retain(|event| *event != egui::ViewportEvent::Close);
    }
}
