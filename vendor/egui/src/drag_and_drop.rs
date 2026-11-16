use alloc::sync::Arc;
use core::any::Any;

use crate::{Context, CursorIcon, Plugin, Pos2, Ui, ViewportId};

/// Plugin for tracking drag-and-drop payload.
///
/// This plugin stores the current drag-and-drop payload internally and handles
/// automatic cleanup when the drag operation ends (via Escape key or mouse release).
///
/// This is a low-level API. For a higher-level API, see:
/// - [`crate::Ui::dnd_drag_source`]
/// - [`crate::Ui::dnd_drop_zone`]
/// - [`crate::Response::dnd_set_drag_payload`]
/// - [`crate::Response::dnd_hover_payload`]
/// - [`crate::Response::dnd_release_payload`]
///
/// This is a built-in plugin in egui, automatically registered during [`Context`] creation.
///
/// See [this example](https://github.com/emilk/egui/blob/main/crates/egui_demo_lib/src/demo/drag_and_drop.rs).
#[doc(alias = "drag and drop")]
#[derive(Clone, Default)]
pub struct DragAndDrop {
    /// The current held operation. UI may publish this after an older release.
    payload: Option<Arc<dyn Any + Send + Sync>>,
    /// Completed operations available only to the pass/viewport that received
    /// their actual endpoint. These are distinct from the new held operation.
    released: Vec<ReleasedPayload>,
}

#[derive(Clone)]
struct ReleasedPayload {
    data: Arc<dyn Any + Send + Sync>,
    pos: Pos2,
    viewport: ViewportId,
}

impl Plugin for DragAndDrop {
    fn debug_name(&self) -> &'static str {
        "DragAndDrop"
    }

    /// Interrupt drag-and-drop if the user presses the escape key.
    ///
    /// This needs to happen at frame start so we can properly capture the escape key.
    fn on_begin_pass(&mut self, ui: &mut Ui) {
        let viewport = ui.ctx().viewport_id();
        let has_any_payload = self.payload.is_some()
            || self
                .released
                .iter()
                .any(|released| released.viewport == viewport);

        if has_any_payload {
            let abort_dnd_due_to_escape_key =
                ui.input_mut(|i| i.consume_key(crate::Modifiers::NONE, crate::Key::Escape));

            if abort_dnd_due_to_escape_key {
                self.payload = None;
                self.released
                    .retain(|released| released.viewport != viewport);
            }
        }
        if let Some((_, position)) = ui.input(|input| input.pointer.button_releases().next())
            && let Some(data) = self.payload.take()
            && let Some(pos) = position
        {
            self.released.push(ReleasedPayload {
                data,
                pos,
                viewport: ui.ctx().viewport_id(),
            });
        }
    }

    /// Interrupt drag-and-drop if the user releases the mouse button.
    ///
    /// This is a catch-all safety net in case user code doesn't capture the drag payload itself.
    /// This must happen at end-of-frame such that we don't shadow the mouse release event from user
    /// code.
    fn on_end_pass(&mut self, ui: &mut Ui) {
        let viewport = ui.ctx().viewport_id();
        self.released
            .retain(|released| released.viewport != viewport);
        if self.payload.is_some() {
            // Only the current held operation controls the grabbing cursor.
            ui.output_mut(|output| {
                if output.cursor_icon == CursorIcon::Default {
                    output.cursor_icon = CursorIcon::Grabbing;
                }
            });
        }
    }
}

impl DragAndDrop {
    /// Set a drag-and-drop payload.
    ///
    /// This can be read by [`Self::payload`] until the pointer is released.
    pub fn set_payload<Payload>(ctx: &Context, payload: Payload)
    where
        Payload: Any + Send + Sync,
    {
        ctx.plugin::<Self>().lock().payload = Some(Arc::new(payload));
    }

    /// Clears the payload, setting it to `None`.
    pub fn clear_payload(ctx: &Context) {
        let plugin = ctx.plugin::<Self>();
        let mut state = plugin.lock();
        state.payload = None;
        state.released.clear();
    }

    /// Retrieve the payload, if any.
    ///
    /// Returns `None` if there is no payload, or if it is not of the requested type.
    ///
    /// Returns `Some` both during a drag and on the frame the pointer is released
    /// (if there is a payload).
    pub fn payload<Payload>(ctx: &Context) -> Option<Arc<Payload>>
    where
        Payload: Any + Send + Sync,
    {
        let viewport = ctx.viewport_id();
        let plugin = ctx.plugin::<Self>();
        let state = plugin.lock();
        let data = state.payload.as_ref().or_else(|| {
            state
                .released
                .iter()
                .find(|released| released.viewport == viewport)
                .map(|released| &released.data)
        })?;
        Arc::clone(data).downcast().ok()
    }

    /// Retrieve and clear the payload, if any.
    ///
    /// Returns `None` if there is no payload, or if it is not of the requested type.
    ///
    /// Returns `Some` both during a drag and on the frame the pointer is released
    /// (if there is a payload).
    pub fn take_payload<Payload>(ctx: &Context) -> Option<Arc<Payload>>
    where
        Payload: Any + Send + Sync,
    {
        let viewport = ctx.viewport_id();
        let plugin = ctx.plugin::<Self>();
        let mut state = plugin.lock();
        let data = state.payload.take().or_else(|| {
            let index = state
                .released
                .iter()
                .position(|released| released.viewport == viewport)?;
            Some(state.released.remove(index).data)
        })?;
        data.downcast().ok()
    }

    pub(crate) fn release_position(ctx: &Context) -> Option<Pos2> {
        let viewport = ctx.viewport_id();
        ctx.plugin::<Self>()
            .lock()
            .released
            .iter()
            .find(|released| released.viewport == viewport)
            .map(|released| released.pos)
    }

    pub(crate) fn take_released_payload<Payload: Any + Send + Sync>(
        ctx: &Context,
    ) -> Option<Arc<Payload>> {
        let viewport = ctx.viewport_id();
        let plugin = ctx.plugin::<Self>();
        let mut state = plugin.lock();
        let index = state
            .released
            .iter()
            .position(|released| released.viewport == viewport)?;
        state.released.remove(index).data.downcast().ok()
    }

    /// Are we carrying a payload of the given type?
    ///
    /// Returns `true` both during a drag and on the frame the pointer is released
    /// (if there is a payload).
    pub fn has_payload_of_type<Payload>(ctx: &Context) -> bool
    where
        Payload: Any + Send + Sync,
    {
        Self::payload::<Payload>(ctx).is_some()
    }

    /// Are we carrying a payload?
    ///
    /// Returns `true` both during a drag and on the frame the pointer is released
    /// (if there is a payload).
    pub fn has_any_payload(ctx: &Context) -> bool {
        let viewport = ctx.viewport_id();
        let plugin = ctx.plugin::<Self>();
        let state = plugin.lock();
        state.payload.is_some()
            || state
                .released
                .iter()
                .any(|released| released.viewport == viewport)
    }
}

#[cfg(test)]
mod release_tests {
    use super::*;
    use crate::{Event, Modifiers, PointerButton, RawInput, pos2};
    fn positioned(pos: Pos2) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }
    }
    fn unknown() -> Event {
        Event::PointerButtonReleased {
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        }
    }
    fn pass(ctx: &Context, events: Vec<Event>, run: impl FnMut(&mut Ui)) {
        ctx.run_ui(
            RawInput {
                events,
                ..Default::default()
            },
            run,
        )
        .drop_without_applying_deltas();
    }
    #[test]
    fn dnd_first_terminal_owns_payload_and_does_not_reuse_later_releases() {
        for first_known in [false, true] {
            let ctx = Context::default();
            DragAndDrop::set_payload(&ctx, 1_u32);
            let events = if first_known {
                vec![
                    positioned(pos2(10.0, 20.0)),
                    unknown(),
                    Event::PointerMoved(pos2(50.0, 60.0)),
                ]
            } else {
                vec![unknown(), positioned(pos2(50.0, 60.0))]
            };
            pass(&ctx, events, |ui| {
                assert_eq!(
                    DragAndDrop::release_position(ui.ctx()),
                    first_known.then_some(pos2(10.0, 20.0))
                );
                assert_eq!(DragAndDrop::payload::<u32>(ui.ctx()).is_some(), first_known);
            });
            assert!(!DragAndDrop::has_any_payload(&ctx));
        }
    }
    #[test]
    fn dnd_new_payload_is_not_dropped_by_the_previous_payloads_terminal() {
        let ctx = Context::default();
        DragAndDrop::set_payload(&ctx, 1_u32);
        pass(&ctx, vec![unknown()], |ui| {
            assert!(!DragAndDrop::has_any_payload(ui.ctx()));
            DragAndDrop::set_payload(ui.ctx(), 2_u32);
            assert_eq!(DragAndDrop::release_position(ui.ctx()), None);
        });
        assert_eq!(*DragAndDrop::payload::<u32>(&ctx).unwrap(), 2);
        pass(&ctx, vec![positioned(pos2(10.0, 20.0))], |ui| {
            assert_eq!(
                DragAndDrop::release_position(ui.ctx()),
                Some(pos2(10.0, 20.0))
            );
            assert_eq!(
                *DragAndDrop::take_released_payload::<u32>(ui.ctx()).unwrap(),
                2
            );
        });
        assert!(!DragAndDrop::has_any_payload(&ctx));
    }
    #[test]
    fn dnd_released_payload_cannot_be_taken_or_retired_by_a_nested_viewport() {
        let ctx = Context::default();
        DragAndDrop::set_payload(&ctx, 1_u32);
        pass(&ctx, vec![positioned(pos2(10.0, 20.0))], |ui| {
            let child = ViewportId::from_hash_of("nested-release");
            let mut raw = RawInput {
                viewport_id: child,
                ..Default::default()
            };
            raw.viewports.insert(
                child,
                crate::ViewportInfo {
                    parent: Some(ViewportId::ROOT),
                    ..Default::default()
                },
            );
            ui.ctx()
                .run_ui(raw, |child_ui| {
                    assert_eq!(DragAndDrop::release_position(child_ui.ctx()), None);
                    assert!(!DragAndDrop::has_any_payload(child_ui.ctx()));
                    assert!(DragAndDrop::payload::<u32>(child_ui.ctx()).is_none());
                    assert!(DragAndDrop::take_released_payload::<u32>(child_ui.ctx()).is_none());
                })
                .drop_without_applying_deltas();
            assert_eq!(
                DragAndDrop::release_position(ui.ctx()),
                Some(pos2(10.0, 20.0))
            );
            assert_eq!(
                *DragAndDrop::take_released_payload::<u32>(ui.ctx()).unwrap(),
                1
            );
        });
    }
}
