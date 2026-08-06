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
/// The edge band is at least 28 pt, or 5% of a wide surface. This fixes the
/// plan's 24–32 pt range at its midpoint while scaling for large tablets.
const EDGE_BAND_MIN_PT: f32 = 28.0;
/// Five percent lets the edge target grow with wide tablet surfaces.
const EDGE_BAND_WIDTH_FRACTION: f32 = 0.05;
/// An edge swipe needs 40 pt of inward travel, the midpoint of the planned
/// 32–48 pt range, so small edge taps cannot open a panel accidentally.
const EDGE_SWIPE_INWARD_PT: f32 = 40.0;
/// Horizontal travel must be at least 1.5 times vertical travel to distinguish
/// a panel gesture from diagonal or vertical canvas movement.
const EDGE_SWIPE_HORIZONTAL_RATIO: f32 = 1.5;
/// Pinch recognition starts at two contacts. Additional contacts are tracked,
/// but only the first two active contacts drive the transform.
const PINCH_CONTACT_COUNT: usize = 2;

/// The center rectangle uses the middle 32% of the surface width.
const CENTER_LEFT_FRACTION: f32 = 0.34;
/// The center rectangle uses the middle 32% of the surface width.
const CENTER_RIGHT_FRACTION: f32 = 0.66;
/// Excluding the top 15% avoids visible top chrome.
const CENTER_TOP_FRACTION: f32 = 0.15;
/// Excluding the bottom 25% avoids the seek bar and leaves page area available.
const CENTER_BOTTOM_FRACTION: f32 = 0.75;

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

    let width = geom.surface.width().max(0.0);
    let height = geom.surface.height().max(0.0);
    let relative_x = if width > 0.0 {
        (pos.x - geom.surface.min.x) / width
    } else {
        0.5
    };
    let relative_y = if height > 0.0 {
        (pos.y - geom.surface.min.y) / height
    } else {
        0.5
    };
    if (CENTER_LEFT_FRACTION..=CENTER_RIGHT_FRACTION).contains(&relative_x)
        && (CENTER_TOP_FRACTION..=CENTER_BOTTOM_FRACTION).contains(&relative_y)
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
    ViewerPointerPassthrough,
    ViewerTapZone,
    Pinch,
    EdgeSwipe { left: bool },
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
    OpenSidePanel {
        left: bool,
    },
    Zoom {
        factor: f32,
        pivot: Pos2,
    },
    Pan {
        delta: Vec2,
    },
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
    edge_side: Option<bool>,
}

impl Contact {
    fn new(sample: TouchSample, surface: Rect) -> Self {
        Self {
            id: sample.id,
            start_pos: sample.pos,
            pos: sample.pos,
            start_ms: sample.now_ms,
            max_distance_sq: 0.0,
            edge_side: edge_side(surface, sample.pos),
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
    suppress_primary: bool,
}

impl Default for TouchRecognizer {
    fn default() -> Self {
        Self {
            contacts: Vec::new(),
            owner: TouchOwner::Undecided,
            pinch_frame: None,
            suppress_primary: false,
        }
    }
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
        if sample.phase == TouchPhase::Cancel {
            self.contacts.clear();
            self.pinch_frame = None;
            self.owner = TouchOwner::Cancelled;
            self.suppress_primary = true;
            return Vec::new();
        }

        match sample.phase {
            TouchPhase::Start => self.handle_start(geom, sample),
            TouchPhase::Move => self.handle_move(sample),
            TouchPhase::End => self.handle_end(geom, sample),
            TouchPhase::Cancel => unreachable!(),
        }
    }

    fn handle_start(&mut self, geom: &TapZoneGeometry, sample: TouchSample) -> Vec<TouchCommand> {
        if self.contacts.is_empty() {
            self.owner = if classify_tap(geom, sample.pos) == TapZone::Excluded {
                TouchOwner::WidgetPassthrough
            } else {
                TouchOwner::Undecided
            };
            self.pinch_frame = None;
            self.suppress_primary = false;
        }

        // Active contact ids are unique. A duplicate Start is ignored rather
        // than being mistaken for a second finger.
        if self.contacts.iter().any(|contact| contact.id == sample.id) {
            return Vec::new();
        }
        self.contacts.push(Contact::new(sample, geom.surface));

        if self.contacts.len() >= PINCH_CONTACT_COUNT {
            match self.owner {
                TouchOwner::Undecided => {
                    self.owner = TouchOwner::Pinch;
                    self.suppress_primary = true;
                    self.rebase_pinch();
                }
                TouchOwner::Pinch => self.rebase_pinch(),
                _ => {}
            }
        }
        Vec::new()
    }

    fn handle_move(&mut self, sample: TouchSample) -> Vec<TouchCommand> {
        let Some(index) = self
            .contacts
            .iter()
            .position(|contact| contact.id == sample.id)
        else {
            return Vec::new();
        };
        self.contacts[index].update(sample.pos);

        if self.owner == TouchOwner::Pinch {
            return self.pinch_commands();
        }
        if self.owner == TouchOwner::Undecided && self.contacts.len() == 1 {
            return self.single_motion_command(self.contacts[index]);
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
        self.contacts[index].update(sample.pos);

        let mut commands = if self.owner == TouchOwner::Pinch {
            self.pinch_commands()
        } else if self.owner == TouchOwner::Undecided && self.contacts.len() == 1 {
            self.single_motion_command(self.contacts[index])
        } else {
            Vec::new()
        };

        if self.owner == TouchOwner::Undecided && self.contacts.len() == 1 {
            let contact = self.contacts[index];
            if contact.is_tap(sample.now_ms) {
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
            } else if contact.max_distance_sq > TAP_MAX_DISTANCE_PT * TAP_MAX_DISTANCE_PT {
                self.owner = TouchOwner::ViewerPointerPassthrough;
            } else {
                // A stationary long press is not a tap, but its synthetic
                // primary release must not become a page command.
                self.suppress_primary = true;
            }
        }

        self.contacts.remove(index);
        if self.owner == TouchOwner::Pinch {
            self.rebase_pinch();
            if self.contacts.is_empty() {
                commands.push(TouchCommand::PinchEnd);
            }
        }
        commands
    }

    fn single_motion_command(&mut self, contact: Contact) -> Vec<TouchCommand> {
        let delta = contact.pos - contact.start_pos;
        let moved_beyond_tap = contact.max_distance_sq > TAP_MAX_DISTANCE_PT * TAP_MAX_DISTANCE_PT;

        if let Some(left) = contact.edge_side {
            let inward = if left { delta.x } else { -delta.x };
            if inward >= EDGE_SWIPE_INWARD_PT
                && delta.x.abs() >= delta.y.abs() * EDGE_SWIPE_HORIZONTAL_RATIO
            {
                self.owner = TouchOwner::EdgeSwipe { left };
                self.suppress_primary = true;
                return vec![TouchCommand::OpenSidePanel { left }];
            }

            // Once movement outside the tap slop is outward or clearly not
            // horizontal, lock into the existing pointer-pan path.
            if moved_beyond_tap
                && (inward <= 0.0 || delta.x.abs() < delta.y.abs() * EDGE_SWIPE_HORIZONTAL_RATIO)
            {
                self.owner = TouchOwner::ViewerPointerPassthrough;
            }
        } else if moved_beyond_tap {
            self.owner = TouchOwner::ViewerPointerPassthrough;
        }
        Vec::new()
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

fn edge_side(surface: Rect, pos: Pos2) -> Option<bool> {
    if !surface.contains(pos) {
        return None;
    }
    let band = EDGE_BAND_MIN_PT.max(surface.width().max(0.0) * EDGE_BAND_WIDTH_FRACTION);
    let near_left = pos.x <= surface.min.x + band;
    let near_right = pos.x >= surface.max.x - band;
    match (near_left, near_right) {
        (true, true) => Some(pos.x < surface.center().x),
        (true, false) => Some(true),
        (false, true) => Some(false),
        (false, false) => None,
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
    fn confirmed_pointer_pan_does_not_switch_owner() {
        let geometry = geom();
        let mut recognizer = TouchRecognizer::new();
        recognizer.handle_sample(&geometry, sample(1, 500.0, 400.0, TouchPhase::Start, 0));
        recognizer.handle_sample(&geometry, sample(1, 520.0, 400.0, TouchPhase::Move, 1));
        recognizer.handle_sample(&geometry, sample(2, 700.0, 400.0, TouchPhase::Start, 2));
        assert_eq!(recognizer.owner(), TouchOwner::ViewerPointerPassthrough);
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
    fn edge_swipe_accepts_only_inward_horizontal_motion_from_the_band() {
        let geometry = geom();

        let mut accepted = TouchRecognizer::new();
        accepted.handle_sample(&geometry, sample(1, 28.0, 400.0, TouchPhase::Start, 0));
        assert_eq!(
            accepted.handle_sample(&geometry, sample(1, 68.0, 410.0, TouchPhase::Move, 10)),
            vec![TouchCommand::OpenSidePanel { left: true }]
        );
        assert_eq!(accepted.owner(), TouchOwner::EdgeSwipe { left: true });

        let mut right = TouchRecognizer::new();
        right.handle_sample(&geometry, sample(2, 975.0, 400.0, TouchPhase::Start, 0));
        assert_eq!(
            right.handle_sample(&geometry, sample(2, 935.0, 400.0, TouchPhase::Move, 10)),
            vec![TouchCommand::OpenSidePanel { left: false }]
        );
    }

    #[test]
    fn edge_swipe_rejects_short_vertical_and_out_of_band_motion() {
        let geometry = geom();
        let cases = [
            ((28.0, 400.0), (67.9, 400.0)),
            ((28.0, 400.0), (68.0, 427.0)),
            ((50.1, 400.0), (100.1, 400.0)),
        ];
        for (index, (start, end)) in cases.into_iter().enumerate() {
            let mut recognizer = TouchRecognizer::new();
            let id = index as u64 + 10;
            recognizer.handle_sample(
                &geometry,
                sample(id, start.0, start.1, TouchPhase::Start, 0),
            );
            let commands =
                recognizer.handle_sample(&geometry, sample(id, end.0, end.1, TouchPhase::End, 10));
            assert!(commands.is_empty());
        }
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
    fn excluded_start_stays_widget_passthrough() {
        let mut geometry = geom();
        geometry
            .excluded
            .push(Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 100.0)));
        let mut recognizer = TouchRecognizer::new();
        recognizer.handle_sample(&geometry, sample(1, 100.0, 50.0, TouchPhase::Start, 0));
        assert_eq!(recognizer.owner(), TouchOwner::WidgetPassthrough);
        assert!(
            recognizer
                .handle_sample(&geometry, sample(1, 500.0, 400.0, TouchPhase::End, 10))
                .is_empty()
        );
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
}
