//! Ordered pointer transport for the three mask-editing canvases.
//!
//! This owns the physical gesture, not the document/Undo payload. Those remain
//! with the existing page-edit/tool owners. A held button cannot start a second
//! tool after a page, layer, mode or context change.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CanvasTool {
    Local(usize, LocalAdjustMaskEditTarget, LocalAdjustMaskTool),
    Erase(EraseTool),
    Conceal(crate::conceal::ConcealTool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CanvasGestureScope {
    context: ViewerContextId,
    page: Option<String>,
    tool: CanvasTool,
}

impl App {
    pub(crate) fn canvas_gesture_scope(&self, idx: usize, tool: CanvasTool) -> CanvasGestureScope {
        CanvasGestureScope {
            context: self.projected_viewer_context_id(),
            page: self.page_path_key(idx),
            tool,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum CanvasPointerInput {
    Press(egui::Pos2, egui::Modifiers),
    Move(egui::Pos2, egui::Modifiers),
    Release(egui::Pos2, egui::Modifiers),
    Secondary(egui::Pos2, egui::Modifiers),
    Hover(Option<egui::Pos2>, egui::Modifiers),
    Cancel,
}

impl CanvasPointerInput {
    pub(crate) fn position(self) -> Option<egui::Pos2> {
        match self {
            Self::Press(p, _) | Self::Move(p, _) | Self::Release(p, _) | Self::Secondary(p, _) => {
                Some(p)
            }
            Self::Hover(p, _) => p,
            Self::Cancel => None,
        }
    }

    pub(crate) fn modifiers(self) -> egui::Modifiers {
        match self {
            Self::Press(_, m)
            | Self::Move(_, m)
            | Self::Release(_, m)
            | Self::Secondary(_, m)
            | Self::Hover(_, m) => m,
            Self::Cancel => egui::Modifiers::NONE,
        }
    }

    pub(crate) fn pressed(self) -> bool {
        matches!(self, Self::Press(..))
    }
    pub(crate) fn released(self) -> bool {
        matches!(self, Self::Release(..))
    }
    // The release edge follows a final Move sample at the release coordinate.
    pub(crate) fn down(self) -> bool {
        matches!(self, Self::Press(..) | Self::Move(..))
    }
    pub(crate) fn secondary(self) -> bool {
        matches!(self, Self::Secondary(..))
    }

    pub(crate) fn primary_sample(self) -> bool {
        self.down() || self.released()
    }
}

#[derive(Clone, Default)]
enum Gesture {
    #[default]
    Idle,
    Pressed {
        scope: CanvasGestureScope,
        position: egui::Pos2,
        modifiers: egui::Modifiers,
    },
}

#[derive(Clone)]
struct CanvasGestureOwner {
    frame: Option<u64>,
    gesture: Gesture,
    focused: bool,
}

impl Default for CanvasGestureOwner {
    fn default() -> Self {
        Self {
            frame: None,
            gesture: Gesture::Idle,
            focused: true,
        }
    }
}

/// Consume this viewport's events once per frame, including discard/re-evaluation
/// passes. Panel/front-UI presses never become canvas gestures. A release belongs
/// only to its accepted press; it can finish outside the image or over a panel.
pub(crate) fn canvas_pointer_inputs(
    ctx: &egui::Context,
    scope: CanvasGestureScope,
    front: crate::ui_fullscreen::FsFrontPointerFrameInput,
    enabled: bool,
    can_start: impl Fn(egui::Pos2) -> bool,
) -> Vec<CanvasPointerInput> {
    let id = egui::Id::unique(("mask-canvas-gesture", ctx.viewport_id()));
    let mut owner = ctx
        .data_mut(|data| data.get_temp::<CanvasGestureOwner>(id))
        .unwrap_or_default();
    let frame = ctx.cumulative_frame_nr();
    if owner.frame == Some(frame) {
        return Vec::new();
    }
    owner.frame = Some(frame);
    // A surviving page may acquire new item slots/generation while its document
    // owner reprojects the same draft. The stable page, not its old index or
    // items generation, owns the pointer gesture too.
    if matches!(&owner.gesture, Gesture::Pressed { scope: old, .. } if old != &scope) {
        owner.gesture = Gesture::Idle;
    }
    let mut inputs = Vec::new();
    let events = front.canvas_events(ctx);
    let (raw_focused, mut modifiers, hover, down) = ctx.input(|i| {
        (
            i.focused,
            i.modifiers,
            i.pointer.hover_pos(),
            i.pointer.primary_down(),
        )
    });
    // Raw focus/modifiers describe the end of the pass. Earlier pointer samples
    // must retain the observations that preceded their ordered change events.
    let mut focused = if events
        .iter()
        .any(|(event, _)| matches!(event, egui::Event::WindowFocused(_)))
    {
        owner.focused
    } else {
        raw_focused
    };
    if let Gesture::Pressed {
        modifiers: previous,
        ..
    } = &owner.gesture
    {
        modifiers = *previous;
    }
    if !enabled || !focused {
        if matches!(owner.gesture, Gesture::Pressed { .. }) {
            inputs.push(CanvasPointerInput::Cancel);
        }
        owner.gesture = Gesture::Idle;
    }
    for (event, owned) in events {
        if owned {
            if matches!(
                event,
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    ..
                } | egui::Event::PointerMoved(_)
                    | egui::Event::PointerGone
            ) && matches!(owner.gesture, Gesture::Pressed { .. })
            {
                inputs.push(CanvasPointerInput::Cancel);
                owner.gesture = Gesture::Idle;
            }
            continue;
        }
        match event {
            egui::Event::ModifiersChanged(m) => {
                modifiers = m;
                if let Gesture::Pressed {
                    position,
                    modifiers: held,
                    ..
                } = &mut owner.gesture
                {
                    *held = m;
                    inputs.push(CanvasPointerInput::Move(*position, m));
                }
            }
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: m,
            } if enabled && focused => {
                modifiers = m;
                if pressed {
                    if can_start(pos) {
                        owner.gesture = Gesture::Pressed {
                            scope: scope.clone(),
                            position: pos,
                            modifiers: m,
                        };
                        inputs.push(CanvasPointerInput::Press(pos, m));
                    } else {
                        owner.gesture = Gesture::Idle;
                    }
                } else if matches!(owner.gesture, Gesture::Pressed { .. }) {
                    inputs.push(CanvasPointerInput::Move(pos, m));
                    inputs.push(CanvasPointerInput::Release(pos, m));
                    owner.gesture = Gesture::Idle;
                }
            }
            egui::Event::PointerMoved(pos) => {
                if let Gesture::Pressed {
                    position,
                    modifiers: held,
                    ..
                } = &mut owner.gesture
                {
                    *position = pos;
                    *held = modifiers;
                    inputs.push(CanvasPointerInput::Move(pos, modifiers));
                }
            }
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: m,
            } if enabled && focused && can_start(pos) => {
                modifiers = m;
                inputs.push(CanvasPointerInput::Secondary(pos, m));
            }
            egui::Event::WindowFocused(true) => focused = true,
            egui::Event::PointerGone | egui::Event::WindowFocused(false) => {
                if matches!(event, egui::Event::WindowFocused(false)) {
                    focused = false;
                }
                if matches!(owner.gesture, Gesture::Pressed { .. }) {
                    inputs.push(CanvasPointerInput::Cancel);
                }
                owner.gesture = Gesture::Idle;
            }
            _ => {}
        }
    }
    // A lost release must never leave a gesture alive. Do not invent a final
    // coordinate or commit an unfinished shape on cancellation.
    if !down && matches!(owner.gesture, Gesture::Pressed { .. }) {
        inputs.push(CanvasPointerInput::Cancel);
        owner.gesture = Gesture::Idle;
    }
    if inputs.is_empty() && enabled && focused && !front.owns(egui::PointerButton::Primary) {
        inputs.push(match &owner.gesture {
            // Keep existing held-drag presentation and modifier-only updates;
            // this is continuation of the accepted press, never a new press.
            Gesture::Pressed { position, .. } => CanvasPointerInput::Move(*position, modifiers),
            Gesture::Idle => CanvasPointerInput::Hover(hover, modifiers),
        });
    }
    owner.focused = focused;
    ctx.data_mut(|data| data.insert_temp(id, owner));
    inputs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section1679_mask_drag_preserves_hold_and_ordered_modifiers() {
        let ctx = egui::Context::default();
        let app = crate::app::setup_app_for_test();
        let scope = app.canvas_gesture_scope(0, CanvasTool::Erase(EraseTool::Brush));
        let a = egui::pos2(10.0, 10.0);
        let b = egui::pos2(20.0, 10.0);
        let c = egui::pos2(30.0, 20.0);
        let mut received = Vec::new();
        let mut collect = |events| {
            let _ = ctx
                .run_pass(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        received = canvas_pointer_inputs(
                            ctx,
                            scope.clone(),
                            Default::default(),
                            true,
                            |_| true,
                        );
                    },
                )
                .drop_without_applying_deltas();
            received.clone()
        };
        let press = collect(vec![egui::Event::PointerButton {
            pos: a,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert!(press[0].pressed());
        let moved = collect(vec![
            egui::Event::PointerMoved(b),
            egui::Event::ModifiersChanged(egui::Modifiers::SHIFT),
            egui::Event::PointerMoved(c),
        ]);
        assert_eq!(moved.len(), 3);
        assert_eq!(moved[0].position(), Some(b));
        assert!(
            !moved[0].modifiers().shift,
            "the final modifier level must not rewrite earlier moves"
        );
        assert_eq!(moved[1].position(), Some(b));
        assert!(moved[1].modifiers().shift);
        assert_eq!(moved[2].position(), Some(c));
        assert!(moved[2].modifiers().shift);
        let held = collect(vec![]);
        assert_eq!(held.len(), 1);
        assert!(held[0].down() && !held[0].pressed());
        assert_eq!(held[0].position(), Some(c));
        let released = collect(vec![egui::Event::PointerButton {
            pos: c,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::SHIFT,
        }]);
        assert_eq!(released.len(), 2);
        assert!(released[0].down() && !released[0].released());
        assert_eq!(released[0].position(), Some(c));
        assert!(released[1].released() && !released[1].down());
    }
}
