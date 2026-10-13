//! How mouse and touch interzcts with widgets.

use crate::{Id, InputState, Key, WidgetRects, hit_test, id, input_state, memory};

use self::{hit_test::WidgetHits, id::IdSet, input_state::PointerEvent, memory::InteractionState};

/// Calculated at the start of each frame
/// based on:
/// * Widget rects from precious frame
/// * Mouse/touch input
/// * Current [`InteractionState`].
#[derive(Clone, Default)]
pub struct InteractionSnapshot {
    /// The widget that got clicked this frame.
    pub clicked: Option<Id>,

    /// The zero-based ordinal of the primary pointer release that assigned [`Self::clicked`].
    ///
    /// Counts all primary releases in this frame, including releases that were not clicks.
    /// A subsequent press does not change this provenance. This is `None` when no widget was
    /// clicked or when the assigned click came from another button or a long touch.
    pub clicked_primary_release_index: Option<usize>,

    /// This widget was long-pressed on a touch screen,
    /// so trigger a secondary click on it (context menu).
    pub long_touched: Option<Id>,

    /// Drag started on this widget this frame.
    ///
    /// This will also be found in `dragged` this frame.
    pub drag_started: Option<Id>,

    /// This widget is being dragged this frame.
    ///
    /// Set the same frame a drag starts,
    /// but unset the frame a drag ends.
    ///
    /// NOTE: this may not have a corresponding [`crate::WidgetRect`],
    /// if this for instance is a drag-and-drop widget which
    /// isn't painted whilst being dragged
    pub dragged: Option<Id>,

    /// This widget was let go this frame,
    /// after having been dragged.
    ///
    /// The widget will not be found in [`Self::dragged`] this frame.
    pub drag_stopped: Option<Id>,

    /// A small set of widgets (usually 0-1) that the pointer is hovering over.
    ///
    /// Show these widgets as highlighted, if they are interactive.
    ///
    /// While dragging or clicking something, nothing else is hovered.
    ///
    /// Use [`Self::contains_pointer`] to find a drop-zone for drag-and-drop.
    pub hovered: IdSet,

    /// All widgets that contain the pointer this frame,
    /// regardless if the user is currently clicking or dragging.
    ///
    /// This is usually a larger set than [`Self::hovered`],
    /// and can be used for e.g. drag-and-drop zones.
    pub contains_pointer: IdSet,
}

impl InteractionSnapshot {
    pub fn ui(&self, ui: &mut crate::Ui) {
        let Self {
            clicked,
            clicked_primary_release_index,
            long_touched,
            drag_started,
            dragged,
            drag_stopped,
            hovered,
            contains_pointer,
        } = self;

        fn id_ui<'a>(ui: &mut crate::Ui, widgets: impl IntoIterator<Item = &'a Id>) {
            for id in widgets {
                ui.label(id.short_debug_format());
            }
        }

        crate::Grid::new("interaction").show(ui, |ui| {
            ui.label("clicked");
            id_ui(ui, clicked);
            ui.end_row();

            ui.label("clicked_primary_release_index");
            ui.label(format!("{clicked_primary_release_index:?}"));
            ui.end_row();

            ui.label("long_touched");
            id_ui(ui, long_touched);
            ui.end_row();

            ui.label("drag_started");
            id_ui(ui, drag_started);
            ui.end_row();

            ui.label("dragged");
            id_ui(ui, dragged);
            ui.end_row();

            ui.label("drag_stopped");
            id_ui(ui, drag_stopped);
            ui.end_row();

            ui.label("hovered");
            id_ui(ui, hovered);
            ui.end_row();

            ui.label("contains_pointer");
            id_ui(ui, contains_pointer);
            ui.end_row();
        });
    }
}

pub(crate) fn interact(
    prev_snapshot: &InteractionSnapshot,
    widgets: &WidgetRects,
    hits: &WidgetHits,
    input: &InputState,
    interaction: &mut InteractionState,
) -> InteractionSnapshot {
    profiling::function_scope!();

    if let Some(id) = interaction.potential_click_id
        && !widgets.contains(id)
    {
        // The widget we were interested in clicking is gone.
        interaction.potential_click_id = None;
    }
    if let Some(id) = interaction.potential_drag_id
        && !widgets.contains(id)
    {
        // The widget we were interested in dragging is gone.
        // This is fine! This could be drag-and-drop,
        // and the widget being dragged is now "in the air" and thus
        // not registered in the new frame.
    }

    let mut clicked = None;
    let mut clicked_primary_release_index = None;
    let mut primary_release_count = 0;
    let mut dragged = prev_snapshot.dragged;
    let mut long_touched = None;

    if input.key_pressed(Key::Escape) {
        // Abort dragging on escape
        dragged = None;
        interaction.potential_drag_id = None;
    }

    if input.is_long_touch() {
        // We implement "press-and-hold for context menu" on touch screens here
        if let Some(widget) = interaction
            .potential_click_id
            .and_then(|id| widgets.get(id))
        {
            dragged = None;
            clicked = Some(widget.id);
            long_touched = Some(widget.id);
            interaction.potential_click_id = None;
            interaction.potential_drag_id = None;
        }
    }

    // Note: in the current code a press-release in the same frame is NOT considered a drag.
    for pointer_event in &input.pointer.pointer_events {
        match pointer_event {
            PointerEvent::Moved(_) => {}

            PointerEvent::Pressed { .. } => {
                // Maybe new click?
                if interaction.potential_click_id.is_none() {
                    interaction.potential_click_id = hits.click.map(|w| w.id);
                }

                // Maybe new drag?
                if interaction.potential_drag_id.is_none() {
                    interaction.potential_drag_id = hits.drag.map(|w| w.id);
                }
            }

            PointerEvent::Released { click, button } => {
                let primary_release_index = if *button == crate::PointerButton::Primary {
                    let index = primary_release_count;
                    primary_release_count += 1;
                    Some(index)
                } else {
                    None
                };

                if click.is_some()
                    && !input.pointer.is_decidedly_dragging()
                    && let Some(widget) = interaction
                        .potential_click_id
                        .and_then(|id| widgets.get(id))
                {
                    clicked = Some(widget.id);
                    clicked_primary_release_index = primary_release_index;
                }

                interaction.potential_drag_id = None;
                interaction.potential_click_id = None;
                dragged = None;
            }
        }
    }

    if dragged.is_none() {
        // Check if we started dragging something new:
        if let Some(widget) = interaction.potential_drag_id.and_then(|id| widgets.get(id))
            && widget.enabled
        {
            let is_dragged = if widget.sense.senses_click() && widget.sense.senses_drag() {
                // This widget is sensitive to both clicks and drags.
                // When the mouse first is pressed, it could be either,
                // so we postpone the decision until we know.
                input.pointer.is_decidedly_dragging()
            } else {
                // This widget is just sensitive to drags, so we can mark it as dragged right away:
                widget.sense.senses_drag()
            };

            if is_dragged {
                dragged = Some(widget.id);
            }
        }
    }

    if !input.pointer.could_any_button_be_click() {
        interaction.potential_click_id = None;
    }

    if !input.pointer.any_down() || input.pointer.latest_pos().is_none() {
        interaction.potential_click_id = None;
        interaction.potential_drag_id = None;
    }

    // ------------------------------------------------------------------------

    let drag_changed = dragged != prev_snapshot.dragged;
    let drag_stopped = drag_changed.then_some(prev_snapshot.dragged).flatten();
    let drag_started = drag_changed.then_some(dragged).flatten();

    // if let Some(drag_started) = drag_started {
    //     eprintln!(
    //         "Started dragging {} {:?}",
    //         drag_started.id.short_debug_format(),
    //         drag_started.rect
    //     );
    // }

    let contains_pointer: IdSet = hits
        .contains_pointer
        .iter()
        .chain(&hits.click)
        .chain(&hits.drag)
        .map(|w| w.id)
        .collect();

    let hovered = if clicked.is_some() || dragged.is_some() || long_touched.is_some() {
        // If currently clicking or dragging, only that and nothing else is hovered.
        clicked
            .iter()
            .chain(&dragged)
            .chain(&long_touched)
            .copied()
            .collect()
    } else {
        // We may be hovering an interactive widget or two.
        // We must also consider the case where non-interactive widgets
        // are _on top_ of an interactive widget.
        // For instance: a label in a draggable window.
        // In that case we want to hover _both_ widgets,
        // otherwise we won't see tooltips for the label.
        //
        // So: we want to hover _all_ widgets above the interactive widget (if any),
        // but none below it (an interactive widget stops the hover search).
        //
        // To know when to stop we need to first know the order of the widgets,
        // which luckily we already have in `hits.close`.

        let order = |id| hits.close.iter().position(|w| w.id == id);

        let click_order = hits.click.and_then(|w| order(w.id)).unwrap_or(0);
        let drag_order = hits.drag.and_then(|w| order(w.id)).unwrap_or(0);
        let top_interactive_order = click_order.max(drag_order);

        let mut hovered: IdSet = hits.click.iter().chain(&hits.drag).map(|w| w.id).collect();

        for w in &hits.contains_pointer {
            let is_interactive = w.sense.senses_click() || w.sense.senses_drag();
            if is_interactive {
                // The only interactive widgets we mark as hovered are the ones
                // in `hits.click` and `hits.drag`!
            } else {
                let is_on_top_of_the_interactive_widget =
                    top_interactive_order <= order(w.id).unwrap_or(0);
                if is_on_top_of_the_interactive_widget {
                    hovered.insert(w.id);
                }
            }
        }

        hovered
    };

    InteractionSnapshot {
        clicked,
        clicked_primary_release_index,
        long_touched,
        drag_started,
        dragged,
        drag_stopped,
        hovered,
        contains_pointer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Event, LayerId, Modifiers, PointerButton, RawInput, Rect, Sense, WidgetRect, pos2,
    };

    fn pointer_button(button: PointerButton, pressed: bool) -> Event {
        Event::PointerButton {
            pos: pos2(10.0, 10.0),
            button,
            pressed,
            modifiers: Modifiers::default(),
        }
    }

    fn next_input(input: InputState, time: f64, events: Vec<Event>) -> InputState {
        input.begin_pass(
            RawInput {
                time: Some(time),
                events,
                ..Default::default()
            },
            false,
            1.0,
            Default::default(),
        )
    }

    fn widgets_and_hits() -> (WidgetRects, WidgetHits, Id, Id) {
        let a = Id::new("click a");
        let b = Id::new("click b");
        let layer_id = LayerId::background();
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(20.0, 20.0));
        let widget = |id| WidgetRect {
            id,
            layer_id,
            rect,
            interact_rect: rect,
            sense: Sense::click(),
            enabled: true,
        };
        let mut widgets = WidgetRects::default();
        widgets.insert(layer_id, widget(a));
        widgets.insert(layer_id, widget(b));
        let hits = WidgetHits {
            click: Some(widget(b)),
            ..Default::default()
        };
        (widgets, hits, a, b)
    }

    #[test]
    fn clicked_primary_release_index_survives_next_press_in_same_frame() {
        let (widgets, hits, a, b) = widgets_and_hits();
        let input = next_input(
            InputState::default(),
            1.0,
            vec![pointer_button(PointerButton::Primary, true)],
        );
        let input = next_input(
            input,
            1.1,
            vec![
                pointer_button(PointerButton::Primary, false),
                pointer_button(PointerButton::Primary, true),
            ],
        );
        let mut interaction = InteractionState {
            potential_click_id: Some(a),
            ..Default::default()
        };

        let snapshot = interact(
            &InteractionSnapshot::default(),
            &widgets,
            &hits,
            &input,
            &mut interaction,
        );

        assert_eq!(snapshot.clicked, Some(a));
        assert_eq!(snapshot.clicked_primary_release_index, Some(0));
        assert_eq!(interaction.potential_click_id, Some(b));
        assert_eq!(snapshot.dragged, None);
    }

    #[test]
    fn clicked_primary_release_index_counts_non_click_releases() {
        let (widgets, hits, _, b) = widgets_and_hits();
        let input = next_input(
            InputState::default(),
            1.0,
            vec![
                pointer_button(PointerButton::Primary, false),
                pointer_button(PointerButton::Primary, true),
                pointer_button(PointerButton::Primary, false),
            ],
        );
        assert!(matches!(
            &input.pointer.pointer_events[0],
            PointerEvent::Released { click: None, .. }
        ));

        let snapshot = interact(
            &InteractionSnapshot::default(),
            &widgets,
            &hits,
            &input,
            &mut InteractionState::default(),
        );

        assert_eq!(snapshot.clicked, Some(b));
        assert_eq!(snapshot.clicked_primary_release_index, Some(1));
    }

    #[test]
    fn clicked_primary_release_index_cleared_by_secondary_click() {
        let (widgets, hits, a, b) = widgets_and_hits();
        let input = next_input(
            InputState::default(),
            1.0,
            vec![pointer_button(PointerButton::Primary, true)],
        );
        let input = next_input(
            input,
            1.1,
            vec![
                pointer_button(PointerButton::Primary, false),
                pointer_button(PointerButton::Secondary, true),
                pointer_button(PointerButton::Secondary, false),
            ],
        );
        let mut interaction = InteractionState {
            potential_click_id: Some(a),
            ..Default::default()
        };

        let snapshot = interact(
            &InteractionSnapshot::default(),
            &widgets,
            &hits,
            &input,
            &mut interaction,
        );

        assert_eq!(snapshot.clicked, Some(b));
        assert_eq!(snapshot.clicked_primary_release_index, None);
    }

    #[test]
    fn clicked_primary_release_index_survives_later_non_click_release() {
        let (widgets, hits, a, _) = widgets_and_hits();
        let input = next_input(
            InputState::default(),
            1.0,
            vec![pointer_button(PointerButton::Primary, true)],
        );
        let input = next_input(
            input,
            1.1,
            vec![
                pointer_button(PointerButton::Primary, false),
                pointer_button(PointerButton::Primary, true),
                Event::PointerMoved(pos2(100.0, 100.0)),
                pointer_button(PointerButton::Primary, false),
            ],
        );
        assert!(matches!(
            input.pointer.pointer_events.last(),
            Some(PointerEvent::Released { click: None, .. })
        ));
        let mut interaction = InteractionState {
            potential_click_id: Some(a),
            ..Default::default()
        };

        let snapshot = interact(
            &InteractionSnapshot::default(),
            &widgets,
            &hits,
            &input,
            &mut interaction,
        );

        assert_eq!(snapshot.clicked, Some(a));
        assert_eq!(snapshot.clicked_primary_release_index, Some(0));
    }

    #[test]
    fn clicked_primary_release_index_not_carried_into_frame_without_click() {
        let (widgets, hits, a, _) = widgets_and_hits();
        let previous = InteractionSnapshot {
            clicked: Some(a),
            clicked_primary_release_index: Some(3),
            ..Default::default()
        };
        let snapshot = interact(
            &previous,
            &widgets,
            &hits,
            &InputState::default(),
            &mut InteractionState::default(),
        );

        assert_eq!(snapshot.clicked, None);
        assert_eq!(snapshot.clicked_primary_release_index, None);
    }

    #[test]
    fn clicked_primary_release_index_none_for_long_touch() {
        let (widgets, hits, a, _) = widgets_and_hits();
        let input = next_input(
            InputState::default(),
            1.0,
            vec![
                Event::Touch {
                    device_id: crate::TouchDeviceId(0),
                    id: crate::TouchId(0),
                    phase: crate::TouchPhase::Start,
                    pos: pos2(10.0, 10.0),
                    force: None,
                },
                pointer_button(PointerButton::Primary, true),
            ],
        );
        let input = next_input(input, 2.0, vec![]);
        assert!(input.is_long_touch());
        let mut interaction = InteractionState {
            potential_click_id: Some(a),
            ..Default::default()
        };

        let snapshot = interact(
            &InteractionSnapshot::default(),
            &widgets,
            &hits,
            &input,
            &mut interaction,
        );

        assert_eq!(snapshot.clicked, Some(a));
        assert_eq!(snapshot.long_touched, Some(a));
        assert_eq!(snapshot.clicked_primary_release_index, None);
        assert_eq!(snapshot.dragged, None);
    }
}
