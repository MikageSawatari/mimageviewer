//! Platform-independent touch gesture recognition.
//!
//! Backends normalize their events into [`TouchSample`] and keep input-source
//! correlation, viewport routing, and command dispatch outside this module.

use egui::{Pos2, Rect, Vec2};

/// A tap may move at most 12 logical points from its start. This is large
/// enough to absorb normal finger jitter without turning a deliberate drag
/// into a tap.
const TAP_MAX_DISTANCE_PT: f32 = 12.0;
/// A tap may last at most 700 ms. Longer contact is reserved for the existing
/// long-press/context-menu path instead of firing a viewer tap command.
const TAP_MAX_DURATION_MS: u64 = 700;
/// Pinch recognition starts at two contacts. Additional contacts are tracked,
/// but only the first two active contacts drive the transform.
const PINCH_CONTACT_COUNT: usize = 2;

/// The center rectangle uses the middle 32% of the surface width.
const CENTER_LEFT_FRACTION: f64 = 0.34;
/// The center rectangle uses the middle 32% of the surface width.
const CENTER_RIGHT_FRACTION: f64 = 0.66;
/// Excluding the top 15% avoids visible top chrome.
const CENTER_TOP_FRACTION: f64 = 0.15;
/// Excluding the bottom 25% avoids the seek bar and leaves page area available.
const CENTER_BOTTOM_FRACTION: f64 = 0.75;

/// Returns the exact center rectangle taught by the still-viewer touch help.
///
/// Keep the classifier and the overlay on this single geometry producer so
/// the visible guide cannot drift away from the actual tap target.
pub(crate) fn center_tap_rect(surface: Rect) -> Rect {
    let width = surface.width().max(0.0);
    let height = surface.height().max(0.0);
    let fraction_coord = |min: f32, extent: f32, fraction: f64| {
        (f64::from(min) + f64::from(extent) * fraction) as f32
    };
    Rect::from_min_max(
        Pos2::new(
            fraction_coord(surface.min.x, width, CENTER_LEFT_FRACTION),
            fraction_coord(surface.min.y, height, CENTER_TOP_FRACTION),
        ),
        Pos2::new(
            fraction_coord(surface.min.x, width, CENTER_RIGHT_FRACTION),
            fraction_coord(surface.min.y, height, CENTER_BOTTOM_FRACTION),
        ),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TouchPhase {
    Start,
    Move,
    End,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TouchSample {
    pub id: u64,
    pub pos: Pos2,
    pub phase: TouchPhase,
    pub now_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TapZoneGeometry {
    pub surface: Rect,
    /// Chrome rectangles actually visible in the current frame.
    pub excluded: Vec<Rect>,
    /// Gesture capabilities differ by surface: the grid owns one-finger
    /// drags for scrolling but may upgrade them to pinch, while viewers can
    /// independently opt in to their existing pointer-pan-to-pinch upgrade.
    pub behavior: TouchSurfaceBehavior,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TouchSurfaceBehavior {
    Grid,
    Viewer { accepts_pinch: bool },
}

impl TouchSurfaceBehavior {
    fn accepts_pinch(self) -> bool {
        matches!(
            self,
            Self::Grid
                | Self::Viewer {
                    accepts_pinch: true
                }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TapZone {
    Center,
    /// The physical screen half. Reading direction is resolved by the caller.
    PageSide {
        left: bool,
    },
    Excluded,
}

/// Classifies a physical tap position without assigning previous/next meaning.
pub(crate) fn classify_tap(geom: &TapZoneGeometry, pos: Pos2) -> TapZone {
    if !geom.surface.contains(pos) || geom.excluded.iter().any(|rect| rect.contains(pos)) {
        return TapZone::Excluded;
    }

    let center = center_tap_rect(geom.surface);
    // `egui::Rect::contains` excludes the maximum edge, while the established
    // center-zone contract includes both fractional boundaries (`..=`).
    // Keep that public behavior while sharing the exact rectangle producer
    // with the visible first-run guide.
    if (center.min.x..=center.max.x).contains(&pos.x)
        && (center.min.y..=center.max.y).contains(&pos.y)
    {
        TapZone::Center
    } else {
        TapZone::PageSide {
            left: pos.x < geom.surface.center().x,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TouchOwner {
    Undecided,
    WidgetPassthrough,
    GridScroll,
    ViewerPointerPassthrough,
    ViewerTapZone,
    Pinch,
    Cancelled,
}

impl Default for TouchOwner {
    fn default() -> Self {
        Self::Undecided
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TouchCommand {
    ToggleChrome,
    /// A physical screen side; the caller applies the reading direction.
    PageSide {
        left: bool,
    },
    Zoom {
        factor: f32,
        pivot: Pos2,
    },
    Pan {
        delta: Vec2,
    },
    /// Incremental vertical finger movement on the grid. The caller converts
    /// this to a snapped anchor plus a fractional drawing remainder.
    ScrollGrid {
        delta_y: f32,
    },
    /// A grid-scroll stream ended because its last contact left or pinch took
    /// ownership. The caller settles the fractional drawing remainder at this
    /// boundary.
    ScrollGridEnd,
    /// The last contact participating in a pinch has ended. Consumers use
    /// this boundary for work that must not run for every gesture sample,
    /// such as a PDF rerender.
    PinchEnd,
}

#[derive(Clone, Copy, Debug)]
struct Contact {
    id: u64,
    start_pos: Pos2,
    pos: Pos2,
    start_ms: u64,
    max_distance_sq: f32,
}

impl Contact {
    fn new(sample: TouchSample) -> Self {
        Self {
            id: sample.id,
            start_pos: sample.pos,
            pos: sample.pos,
            start_ms: sample.now_ms,
            max_distance_sq: 0.0,
        }
    }

    fn update(&mut self, pos: Pos2) {
        self.pos = pos;
        self.max_distance_sq = self
            .max_distance_sq
            .max((self.pos - self.start_pos).length_sq());
    }

    fn is_tap(self, now_ms: u64) -> bool {
        self.max_distance_sq <= TAP_MAX_DISTANCE_PT * TAP_MAX_DISTANCE_PT
            && now_ms.saturating_sub(self.start_ms) <= TAP_MAX_DURATION_MS
    }
}

#[derive(Clone, Copy, Debug)]
struct PinchFrame {
    ids: [u64; PINCH_CONTACT_COUNT],
    positions: [Pos2; PINCH_CONTACT_COUNT],
}

/// Stateful recognizer for one touch surface.
///
/// Completed ownership is retained until the next `Start`, allowing the input
/// adapter to query suppression after processing the final `End` in a frame.
#[derive(Clone, Debug)]
pub(crate) struct TouchRecognizer {
    contacts: Vec<Contact>,
    owner: TouchOwner,
    pinch_frame: Option<PinchFrame>,
    grid_scroll_contact_id: Option<u64>,
    suppress_primary: bool,
}

impl Default for TouchRecognizer {
    fn default() -> Self {
        Self {
            contacts: Vec::new(),
            owner: TouchOwner::Undecided,
            pinch_frame: None,
            grid_scroll_contact_id: None,
            suppress_primary: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TouchStartPolicy {
    ClassifyGeometry,
    WidgetPassthrough,
}

impl TouchRecognizer {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn owner(&self) -> TouchOwner {
        self.owner
    }

    pub(crate) fn is_active(&self) -> bool {
        !self.contacts.is_empty()
    }

    pub(crate) fn contact_count(&self) -> usize {
        self.contacts.len()
    }

    /// Whether the correlated synthetic primary click must be suppressed for
    /// the current (or just-completed) touch stream.
    pub(crate) fn should_suppress_primary(&self) -> bool {
        self.suppress_primary
    }

    pub(crate) fn handle_sample(
        &mut self,
        geom: &TapZoneGeometry,
        sample: TouchSample,
    ) -> Vec<TouchCommand> {
        self.handle_sample_with_start_policy(geom, sample, TouchStartPolicy::ClassifyGeometry)
    }

    /// Handles a contact delivered by a window whose OS hit-test has already
    /// established widget ownership.
    ///
    /// Unlike [`Self::handle_sample`], the first contact bypasses
    /// [`classify_tap`] entirely. This is used by the native video HUD HWND:
    /// receiving the pointer there is itself the authoritative hit-test, so a
    /// presenter-side approximation must not reclassify it as a viewer tap.
    pub(crate) fn handle_widget_passthrough_sample(
        &mut self,
        geom: &TapZoneGeometry,
        sample: TouchSample,
    ) -> Vec<TouchCommand> {
        self.handle_sample_with_start_policy(geom, sample, TouchStartPolicy::WidgetPassthrough)
    }

    fn handle_sample_with_start_policy(
        &mut self,
        geom: &TapZoneGeometry,
        sample: TouchSample,
        start_policy: TouchStartPolicy,
    ) -> Vec<TouchCommand> {
        if sample.phase == TouchPhase::Cancel {
            self.contacts.clear();
            self.pinch_frame = None;
            self.grid_scroll_contact_id = None;
            self.owner = TouchOwner::Cancelled;
            self.suppress_primary = true;
            return Vec::new();
        }

        match sample.phase {
            TouchPhase::Start => self.handle_start(geom, sample, start_policy),
            TouchPhase::Move => self.handle_move(geom, sample),
            TouchPhase::End => self.handle_end(geom, sample),
            TouchPhase::Cancel => unreachable!(),
        }
    }

    fn handle_start(
        &mut self,
        geom: &TapZoneGeometry,
        sample: TouchSample,
        start_policy: TouchStartPolicy,
    ) -> Vec<TouchCommand> {
        if self.contacts.is_empty() {
            self.owner = match start_policy {
                TouchStartPolicy::ClassifyGeometry => {
                    if classify_tap(geom, sample.pos) == TapZone::Excluded {
                        TouchOwner::WidgetPassthrough
                    } else {
                        TouchOwner::Undecided
                    }
                }
                TouchStartPolicy::WidgetPassthrough => TouchOwner::WidgetPassthrough,
            };
            self.pinch_frame = None;
            self.grid_scroll_contact_id = None;
            self.suppress_primary = false;
        }

        // Active contact ids are unique. A duplicate Start is ignored rather
        // than being mistaken for a second finger.
        if self.contacts.iter().any(|contact| contact.id == sample.id) {
            return Vec::new();
        }
        self.contacts.push(Contact::new(sample));

        let mut commands = Vec::new();
        if self.contacts.len() >= PINCH_CONTACT_COUNT && geom.behavior.accepts_pinch() {
            match self.owner {
                TouchOwner::Undecided | TouchOwner::ViewerPointerPassthrough => self.begin_pinch(),
                TouchOwner::GridScroll => {
                    // Pinch takes exclusive ownership from this point. End the
                    // fractional grid drag at the ownership boundary so the
                    // caller can settle it before interpreting Zoom samples.
                    self.begin_pinch();
                    commands.push(TouchCommand::ScrollGridEnd);
                }
                TouchOwner::Pinch => self.rebase_pinch(),
                TouchOwner::WidgetPassthrough
                | TouchOwner::ViewerTapZone
                | TouchOwner::Cancelled => {}
            }
        }
        commands
    }

    fn begin_pinch(&mut self) {
        self.owner = TouchOwner::Pinch;
        self.grid_scroll_contact_id = None;
        self.suppress_primary = true;
        self.rebase_pinch();
    }

    fn handle_move(&mut self, geom: &TapZoneGeometry, sample: TouchSample) -> Vec<TouchCommand> {
        let Some(index) = self
            .contacts
            .iter()
            .position(|contact| contact.id == sample.id)
        else {
            return Vec::new();
        };
        let previous_pos = self.contacts[index].pos;
        self.contacts[index].update(sample.pos);

        if self.owner == TouchOwner::Pinch {
            return self.pinch_commands();
        }
        if self.owner == TouchOwner::GridScroll {
            return self.grid_scroll_command(sample.id, sample.pos.y - previous_pos.y);
        }
        if self.owner == TouchOwner::Undecided && self.contacts.len() == 1 {
            return self.single_motion_command(geom.behavior, self.contacts[index]);
        }
        Vec::new()
    }

    fn handle_end(&mut self, geom: &TapZoneGeometry, sample: TouchSample) -> Vec<TouchCommand> {
        let Some(index) = self
            .contacts
            .iter()
            .position(|contact| contact.id == sample.id)
        else {
            return Vec::new();
        };
        let previous_pos = self.contacts[index].pos;
        self.contacts[index].update(sample.pos);

        let mut commands = if self.owner == TouchOwner::Pinch {
            self.pinch_commands()
        } else if self.owner == TouchOwner::GridScroll {
            self.grid_scroll_command(sample.id, sample.pos.y - previous_pos.y)
        } else if self.owner == TouchOwner::Undecided && self.contacts.len() == 1 {
            self.single_motion_command(geom.behavior, self.contacts[index])
        } else {
            Vec::new()
        };

        if self.owner == TouchOwner::Undecided && self.contacts.len() == 1 {
            let contact = self.contacts[index];
            if contact.is_tap(sample.now_ms) {
                match geom.behavior {
                    TouchSurfaceBehavior::Grid => {
                        // Grid taps stay on egui's existing cell selection /
                        // double-click path. Re-tap open is a separate step.
                        self.owner = TouchOwner::WidgetPassthrough;
                    }
                    TouchSurfaceBehavior::Viewer { .. } => {
                        match classify_tap(geom, contact.pos) {
                            TapZone::Center => {
                                self.owner = TouchOwner::ViewerTapZone;
                                self.suppress_primary = true;
                                commands.push(TouchCommand::ToggleChrome);
                            }
                            TapZone::PageSide { left } => {
                                self.owner = TouchOwner::ViewerTapZone;
                                self.suppress_primary = true;
                                commands.push(TouchCommand::PageSide { left });
                            }
                            TapZone::Excluded => {
                                // The press began on the viewer, so do not pass its
                                // release through to chrome reached during the tap.
                                self.suppress_primary = true;
                            }
                        }
                    }
                }
            } else if contact.max_distance_sq > TAP_MAX_DISTANCE_PT * TAP_MAX_DISTANCE_PT {
                self.owner = match geom.behavior {
                    TouchSurfaceBehavior::Grid => TouchOwner::GridScroll,
                    TouchSurfaceBehavior::Viewer { .. } => TouchOwner::ViewerPointerPassthrough,
                };
            } else {
                match geom.behavior {
                    TouchSurfaceBehavior::Grid => {
                        self.owner = TouchOwner::WidgetPassthrough;
                    }
                    TouchSurfaceBehavior::Viewer { .. } => {
                        // A stationary long press is not a tap, but its synthetic
                        // primary release must not become a page command.
                        self.suppress_primary = true;
                    }
                }
            }
        }

        self.contacts.remove(index);
        match self.owner {
            TouchOwner::Pinch => {
                self.rebase_pinch();
                if self.contacts.is_empty() {
                    commands.push(TouchCommand::PinchEnd);
                }
            }
            TouchOwner::GridScroll if self.contacts.is_empty() => {
                self.grid_scroll_contact_id = None;
                commands.push(TouchCommand::ScrollGridEnd);
            }
            _ => {}
        }
        commands
    }

    fn single_motion_command(
        &mut self,
        behavior: TouchSurfaceBehavior,
        contact: Contact,
    ) -> Vec<TouchCommand> {
        let delta = contact.pos - contact.start_pos;
        let moved_beyond_tap = contact.max_distance_sq > TAP_MAX_DISTANCE_PT * TAP_MAX_DISTANCE_PT;

        if behavior == TouchSurfaceBehavior::Grid {
            if moved_beyond_tap {
                self.owner = TouchOwner::GridScroll;
                self.grid_scroll_contact_id = Some(contact.id);
                self.suppress_primary = true;
                if delta.y != 0.0 {
                    return vec![TouchCommand::ScrollGrid { delta_y: delta.y }];
                }
            }
            return Vec::new();
        }

        if moved_beyond_tap {
            self.owner = TouchOwner::ViewerPointerPassthrough;
        }
        Vec::new()
    }

    fn grid_scroll_command(&self, contact_id: u64, delta_y: f32) -> Vec<TouchCommand> {
        if self.grid_scroll_contact_id == Some(contact_id) && delta_y != 0.0 {
            vec![TouchCommand::ScrollGrid { delta_y }]
        } else {
            Vec::new()
        }
    }

    fn pinch_commands(&mut self) -> Vec<TouchCommand> {
        let Some(previous) = self.pinch_frame else {
            self.rebase_pinch();
            return Vec::new();
        };
        let Some(current) = self.pinch_frame_for_ids(previous.ids) else {
            self.rebase_pinch();
            return Vec::new();
        };

        let old_vector = previous.positions[1] - previous.positions[0];
        let new_vector = current.positions[1] - current.positions[0];
        let old_pivot = previous.positions[0] + old_vector * 0.5;
        let new_pivot = current.positions[0] + new_vector * 0.5;
        let old_distance = old_vector.length();
        let new_distance = new_vector.length();
        self.pinch_frame = Some(current);

        let mut commands = Vec::with_capacity(2);
        if old_distance > 0.0 && new_distance > 0.0 {
            let factor = new_distance / old_distance;
            if factor.is_finite() && factor != 1.0 {
                commands.push(TouchCommand::Zoom {
                    factor,
                    pivot: new_pivot,
                });
            }
        }
        let delta = new_pivot - old_pivot;
        if delta != Vec2::ZERO {
            commands.push(TouchCommand::Pan { delta });
        }
        commands
    }

    fn rebase_pinch(&mut self) {
        self.pinch_frame = if self.contacts.len() >= PINCH_CONTACT_COUNT {
            Some(PinchFrame {
                ids: [self.contacts[0].id, self.contacts[1].id],
                positions: [self.contacts[0].pos, self.contacts[1].pos],
            })
        } else {
            None
        };
    }

    fn pinch_frame_for_ids(&self, ids: [u64; PINCH_CONTACT_COUNT]) -> Option<PinchFrame> {
        let first = self.contacts.iter().find(|contact| contact.id == ids[0])?;
        let second = self.contacts.iter().find(|contact| contact.id == ids[1])?;
        Some(PinchFrame {
            ids,
            positions: [first.pos, second.pos],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    fn geom() -> TapZoneGeometry {
        TapZoneGeometry {
            surface: Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 800.0)),
            excluded: Vec::new(),
            behavior: TouchSurfaceBehavior::Viewer {
                accepts_pinch: true,
            },
        }
    }

    fn grid_geom() -> TapZoneGeometry {
        TapZoneGeometry {
            behavior: TouchSurfaceBehavior::Grid,
            ..geom()
        }
    }

    fn sample(id: u64, x: f32, y: f32, phase: TouchPhase, now_ms: u64) -> TouchSample {
        TouchSample {
            id,
            pos: pos2(x, y),
            phase,
            now_ms,
        }
    }

    #[test]
    fn tap_zone_classifies_center_sides_exclusions_and_boundaries() {
        let mut geometry = geom();
        geometry
            .excluded
            .push(Rect::from_min_max(pos2(900.0, 0.0), pos2(1000.0, 100.0)));

        assert_eq!(classify_tap(&geometry, pos2(500.0, 400.0)), TapZone::Center);
        assert_eq!(
            classify_tap(&geometry, pos2(100.0, 400.0)),
            TapZone::PageSide { left: true }
        );
        assert_eq!(
            classify_tap(&geometry, pos2(800.0, 400.0)),
            TapZone::PageSide { left: false }
        );
        assert_eq!(
            classify_tap(&geometry, pos2(950.0, 50.0)),
            TapZone::Excluded
        );

        // Rect boundaries are inclusive: 34%..66% and 15%..75% belong to Center.
        assert_eq!(classify_tap(&geometry, pos2(340.0, 120.0)), TapZone::Center);
        assert_eq!(classify_tap(&geometry, pos2(660.0, 600.0)), TapZone::Center);
        assert_eq!(
            classify_tap(&geometry, pos2(339.9, 120.0)),
            TapZone::PageSide { left: true }
        );
        assert_eq!(
            classify_tap(&geometry, pos2(660.1, 600.0)),
            TapZone::PageSide { left: false }
        );
        assert_eq!(
            classify_tap(&geometry, pos2(-0.1, 400.0)),
            TapZone::Excluded
        );
    }

    #[test]
    fn excluded_rect_wins_when_it_overlaps_center() {
        let mut geometry = geom();
        geometry
            .excluded
            .push(Rect::from_min_max(pos2(450.0, 350.0), pos2(550.0, 450.0)));
        assert_eq!(
            classify_tap(&geometry, pos2(500.0, 400.0)),
            TapZone::Excluded
        );
    }

    #[test]
    fn center_rect_remains_valid_on_tiny_and_extreme_surfaces() {
        for surface in [
            Rect::from_min_max(pos2(10.0, 20.0), pos2(11.0, 21.0)),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(2.0, 100_000.0)),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(100_000.0, 2.0)),
        ] {
            let geometry = TapZoneGeometry {
                surface,
                excluded: Vec::new(),
                behavior: TouchSurfaceBehavior::Viewer {
                    accepts_pinch: true,
                },
            };
            assert_eq!(classify_tap(&geometry, surface.center()), TapZone::Center);
        }
    }

    #[test]
    fn tap_thresholds_are_inclusive_and_use_maximum_displacement() {
        let geometry = geom();
        let mut recognizer = TouchRecognizer::new();
        recognizer.handle_sample(&geometry, sample(1, 500.0, 400.0, TouchPhase::Start, 100));
        let commands =
            recognizer.handle_sample(&geometry, sample(1, 512.0, 400.0, TouchPhase::End, 800));
        assert_eq!(commands, vec![TouchCommand::ToggleChrome]);

        let mut recognizer = TouchRecognizer::new();
        recognizer.handle_sample(&geometry, sample(2, 500.0, 400.0, TouchPhase::Start, 0));
        recognizer.handle_sample(&geometry, sample(2, 512.1, 400.0, TouchPhase::Move, 100));
        let commands =
            recognizer.handle_sample(&geometry, sample(2, 500.0, 400.0, TouchPhase::End, 200));
        assert!(commands.is_empty());
        assert_eq!(recognizer.owner(), TouchOwner::ViewerPointerPassthrough);

        let mut recognizer = TouchRecognizer::new();
        recognizer.handle_sample(&geometry, sample(3, 500.0, 400.0, TouchPhase::Start, 0));
        let commands =
            recognizer.handle_sample(&geometry, sample(3, 500.0, 400.0, TouchPhase::End, 701));
        assert!(commands.is_empty());
        assert!(recognizer.should_suppress_primary());
    }

    #[test]
    fn second_contact_cancels_tap_even_when_first_finger_ends_first() {
        let geometry = geom();
        let mut recognizer = TouchRecognizer::new();
        recognizer.handle_sample(&geometry, sample(24, 200.0, 400.0, TouchPhase::Start, 0));
        recognizer.handle_sample(&geometry, sample(57, 800.0, 400.0, TouchPhase::Start, 10));
        assert_eq!(recognizer.owner(), TouchOwner::Pinch);
        assert!(recognizer.should_suppress_primary());

        assert!(
            recognizer
                .handle_sample(&geometry, sample(24, 200.0, 400.0, TouchPhase::End, 20))
                .is_empty()
        );
        assert!(recognizer.is_active());
        assert_eq!(recognizer.owner(), TouchOwner::Pinch);
        assert_eq!(
            recognizer.handle_sample(&geometry, sample(57, 800.0, 400.0, TouchPhase::End, 30)),
            vec![TouchCommand::PinchEnd]
        );
        assert!(!recognizer.is_active());
        assert_eq!(recognizer.owner(), TouchOwner::Pinch);
        assert!(recognizer.should_suppress_primary());
    }

    #[test]
    fn confirmed_pointer_pan_upgrades_to_rebased_pinch_when_contact_is_added() {
        let geometry = geom();
        let mut recognizer = TouchRecognizer::new();
        recognizer.handle_sample(&geometry, sample(1, 200.0, 400.0, TouchPhase::Start, 0));
        recognizer.handle_sample(&geometry, sample(1, 600.0, 400.0, TouchPhase::Move, 1));
        assert_eq!(recognizer.owner(), TouchOwner::ViewerPointerPassthrough);

        assert!(
            recognizer
                .handle_sample(&geometry, sample(2, 700.0, 400.0, TouchPhase::Start, 2))
                .is_empty()
        );
        assert_eq!(recognizer.owner(), TouchOwner::Pinch);
        assert!(recognizer.should_suppress_primary());

        // The 400 pt single-finger pan must not participate in the pinch
        // delta. The first pinch sample is incremental from the upgrade.
        assert_eq!(
            recognizer.handle_sample(&geometry, sample(1, 601.0, 400.0, TouchPhase::Move, 3)),
            vec![
                TouchCommand::Zoom {
                    factor: 0.99,
                    pivot: pos2(650.5, 400.0),
                },
                TouchCommand::Pan {
                    delta: vec2(0.5, 0.0),
                },
            ]
        );
    }

    #[test]
    fn cancel_discards_the_whole_stream_and_emits_nothing() {
        let geometry = geom();
        let mut recognizer = TouchRecognizer::new();
        recognizer.handle_sample(&geometry, sample(1, 200.0, 300.0, TouchPhase::Start, 0));
        recognizer.handle_sample(&geometry, sample(2, 800.0, 300.0, TouchPhase::Start, 1));
        assert!(
            recognizer
                .handle_sample(&geometry, sample(1, 210.0, 300.0, TouchPhase::Cancel, 2))
                .is_empty()
        );
        assert!(!recognizer.is_active());
        assert_eq!(recognizer.owner(), TouchOwner::Cancelled);
        assert!(recognizer.should_suppress_primary());
        assert!(
            recognizer
                .handle_sample(&geometry, sample(2, 800.0, 300.0, TouchPhase::End, 3))
                .is_empty()
        );
    }

    #[test]
    fn large_discontinuous_ids_and_third_contact_are_supported() {
        let geometry = geom();
        let mut recognizer = TouchRecognizer::new();
        let first = 9_000_000_000;
        let second = 42_424_242_424;
        let third = u64::MAX - 1;
        recognizer.handle_sample(&geometry, sample(first, 100.0, 100.0, TouchPhase::Start, 0));
        recognizer.handle_sample(
            &geometry,
            sample(second, 200.0, 100.0, TouchPhase::Start, 1),
        );
        recognizer.handle_sample(&geometry, sample(third, 300.0, 100.0, TouchPhase::Start, 2));

        // Only the first two active contacts drive the transform.
        assert!(
            recognizer
                .handle_sample(&geometry, sample(third, 350.0, 100.0, TouchPhase::Move, 3))
                .is_empty()
        );
        recognizer.handle_sample(&geometry, sample(first, 100.0, 100.0, TouchPhase::End, 4));
        let commands =
            recognizer.handle_sample(&geometry, sample(third, 500.0, 100.0, TouchPhase::Move, 5));
        assert_eq!(
            commands,
            vec![
                TouchCommand::Zoom {
                    factor: 2.0,
                    pivot: pos2(350.0, 100.0)
                },
                TouchCommand::Pan {
                    delta: vec2(75.0, 0.0)
                },
            ]
        );
    }

    #[test]
    fn excluded_start_stays_widget_passthrough_when_contact_is_added() {
        let mut geometry = geom();
        geometry
            .excluded
            .push(Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 100.0)));
        let mut recognizer = TouchRecognizer::new();
        recognizer.handle_sample(&geometry, sample(1, 100.0, 50.0, TouchPhase::Start, 0));
        assert_eq!(recognizer.owner(), TouchOwner::WidgetPassthrough);
        assert!(
            recognizer
                .handle_sample(&geometry, sample(2, 700.0, 400.0, TouchPhase::Start, 1))
                .is_empty()
        );
        assert_eq!(recognizer.owner(), TouchOwner::WidgetPassthrough);
        assert!(
            recognizer
                .handle_sample(&geometry, sample(2, 800.0, 400.0, TouchPhase::Move, 2))
                .is_empty()
        );
        assert!(!recognizer.should_suppress_primary());
        assert!(
            recognizer
                .handle_sample(&geometry, sample(1, 500.0, 400.0, TouchPhase::End, 10))
                .is_empty()
        );
        assert!(!recognizer.should_suppress_primary());
    }

    #[test]
    fn upgraded_pinch_does_not_downgrade_and_next_stream_resets() {
        let geometry = geom();
        let mut recognizer = TouchRecognizer::new();
        recognizer.handle_sample(&geometry, sample(1, 200.0, 400.0, TouchPhase::Start, 0));
        recognizer.handle_sample(&geometry, sample(1, 400.0, 400.0, TouchPhase::Move, 1));
        recognizer.handle_sample(&geometry, sample(2, 700.0, 400.0, TouchPhase::Start, 2));
        assert_eq!(recognizer.owner(), TouchOwner::Pinch);

        assert!(
            recognizer
                .handle_sample(&geometry, sample(2, 700.0, 400.0, TouchPhase::End, 3))
                .is_empty()
        );
        assert!(recognizer.is_active());
        assert_eq!(recognizer.owner(), TouchOwner::Pinch);
        assert!(
            recognizer
                .handle_sample(&geometry, sample(1, 450.0, 400.0, TouchPhase::Move, 4))
                .is_empty()
        );
        assert_eq!(recognizer.owner(), TouchOwner::Pinch);
        assert_eq!(
            recognizer.handle_sample(&geometry, sample(1, 450.0, 400.0, TouchPhase::End, 5)),
            vec![TouchCommand::PinchEnd]
        );
        assert!(!recognizer.is_active());

        recognizer.handle_sample(&geometry, sample(3, 500.0, 400.0, TouchPhase::Start, 6));
        assert_eq!(recognizer.owner(), TouchOwner::Undecided);
        assert!(!recognizer.should_suppress_primary());
    }

    #[test]
    fn pinch_emits_incremental_zoom_and_pan_and_never_returns_to_tap() {
        let geometry = geom();
        let mut recognizer = TouchRecognizer::new();
        recognizer.handle_sample(&geometry, sample(100, 100.0, 200.0, TouchPhase::Start, 0));
        recognizer.handle_sample(&geometry, sample(900, 110.0, 200.0, TouchPhase::Start, 1));
        let commands =
            recognizer.handle_sample(&geometry, sample(900, 120.0, 200.0, TouchPhase::Move, 2));
        assert_eq!(
            commands,
            vec![
                TouchCommand::Zoom {
                    factor: 2.0,
                    pivot: pos2(110.0, 200.0)
                },
                TouchCommand::Pan {
                    delta: vec2(5.0, 0.0)
                },
            ]
        );

        recognizer.handle_sample(&geometry, sample(900, 120.0, 200.0, TouchPhase::End, 3));
        assert_eq!(recognizer.owner(), TouchOwner::Pinch);
        assert_eq!(
            recognizer.handle_sample(&geometry, sample(100, 100.0, 200.0, TouchPhase::End, 4)),
            vec![TouchCommand::PinchEnd]
        );
        assert_eq!(recognizer.owner(), TouchOwner::Pinch);
    }

    #[test]
    fn grid_tap_stays_on_the_existing_widget_path() {
        let geometry = grid_geom();
        let mut recognizer = TouchRecognizer::new();
        recognizer.handle_sample(&geometry, sample(1, 500.0, 400.0, TouchPhase::Start, 0));

        assert!(
            recognizer
                .handle_sample(&geometry, sample(1, 505.0, 404.0, TouchPhase::End, 100))
                .is_empty()
        );
        assert_eq!(recognizer.owner(), TouchOwner::WidgetPassthrough);
        assert!(!recognizer.should_suppress_primary());
    }

    #[test]
    fn grid_drag_claims_after_tap_slop_and_emits_incremental_vertical_motion() {
        let geometry = grid_geom();
        let mut recognizer = TouchRecognizer::new();
        recognizer.handle_sample(&geometry, sample(1, 500.0, 400.0, TouchPhase::Start, 0));

        assert!(
            recognizer
                .handle_sample(&geometry, sample(1, 500.0, 412.0, TouchPhase::Move, 10))
                .is_empty()
        );
        assert_eq!(recognizer.owner(), TouchOwner::Undecided);
        assert_eq!(
            recognizer.handle_sample(&geometry, sample(1, 500.0, 413.0, TouchPhase::Move, 20)),
            vec![TouchCommand::ScrollGrid { delta_y: 13.0 }]
        );
        assert_eq!(recognizer.owner(), TouchOwner::GridScroll);
        assert!(recognizer.should_suppress_primary());
        assert_eq!(
            recognizer.handle_sample(&geometry, sample(1, 500.0, 433.0, TouchPhase::Move, 30)),
            vec![TouchCommand::ScrollGrid { delta_y: 20.0 }]
        );
        assert_eq!(
            recognizer.handle_sample(&geometry, sample(1, 500.0, 438.0, TouchPhase::End, 40)),
            vec![
                TouchCommand::ScrollGrid { delta_y: 5.0 },
                TouchCommand::ScrollGridEnd,
            ]
        );
    }

    #[test]
    fn grid_scroll_upgrades_to_pinch_and_stops_emitting_scroll() {
        let geometry = grid_geom();
        let mut recognizer = TouchRecognizer::new();
        recognizer.handle_sample(&geometry, sample(1, 500.0, 400.0, TouchPhase::Start, 0));
        assert_eq!(
            recognizer.handle_sample(&geometry, sample(1, 500.0, 380.0, TouchPhase::Move, 10)),
            vec![TouchCommand::ScrollGrid { delta_y: -20.0 }]
        );

        assert_eq!(
            recognizer.handle_sample(&geometry, sample(2, 600.0, 400.0, TouchPhase::Start, 20)),
            vec![TouchCommand::ScrollGridEnd]
        );
        assert_eq!(recognizer.owner(), TouchOwner::Pinch);
        let pinch_commands =
            recognizer.handle_sample(&geometry, sample(2, 650.0, 360.0, TouchPhase::Move, 30));
        assert!(
            pinch_commands
                .iter()
                .any(|command| matches!(command, TouchCommand::Zoom { .. }))
        );
        assert!(
            pinch_commands
                .iter()
                .all(|command| !matches!(command, TouchCommand::ScrollGrid { .. }))
        );
        assert_eq!(recognizer.owner(), TouchOwner::Pinch);

        assert!(
            recognizer
                .handle_sample(&geometry, sample(1, 500.0, 380.0, TouchPhase::End, 40))
                .is_empty()
        );
        assert_eq!(recognizer.owner(), TouchOwner::Pinch);
        assert_eq!(
            recognizer.handle_sample(&geometry, sample(2, 650.0, 360.0, TouchPhase::End, 50)),
            vec![TouchCommand::PinchEnd]
        );
    }
}
