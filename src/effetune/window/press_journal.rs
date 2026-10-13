//! Correspondence between native input, pre-queue observations, and egui clicks.
//! Nothing here queries the OS or changes input delivery.

use std::collections::{HashMap, HashSet, VecDeque};

use super::press_capture::{CaptureRecord, ForegroundFact, PacketKey};

const LIMIT: usize = 512;

#[derive(Default)]
pub(super) struct Journal {
    entries: HashMap<PacketKey, (ForegroundFact, bool)>,
    ambiguous: HashSet<PacketKey>,
    lost: bool,
}

impl Journal {
    pub fn insert(&mut self, record: CaptureRecord) {
        if self.lost {
            return;
        }
        if self.entries.contains_key(&record.key) || self.ambiguous.contains(&record.key) {
            self.entries.remove(&record.key);
            self.ambiguous.insert(record.key);
        } else {
            self.entries.insert(record.key, (record.foreground, false));
        }
        if self.entries.len() + self.ambiguous.len() > LIMIT {
            self.invalidate();
        }
    }

    pub fn claim(&mut self, key: PacketKey) -> ForegroundFact {
        if self.lost || self.ambiguous.contains(&key) {
            return ForegroundFact::Unknown;
        }
        match self.entries.get_mut(&key) {
            Some((fact, claimed)) if !*claimed => {
                *claimed = true;
                fact.clone()
            }
            Some(_) => {
                self.entries.remove(&key);
                self.ambiguous.insert(key);
                ForegroundFact::Unknown
            }
            None => ForegroundFact::Unknown,
        }
    }

    pub fn still_unique(&self, key: PacketKey) -> bool {
        !self.lost && !self.ambiguous.contains(&key)
    }

    pub fn retire(&mut self, key: PacketKey) {
        // Ambiguity is sticky: another queued packet of that cohort may arrive.
        self.entries.remove(&key);
    }

    pub fn invalidate(&mut self) {
        self.lost = true;
        self.entries.clear();
        self.ambiguous.clear();
    }

    pub fn lost(&self) -> bool {
        self.lost
    }
}

#[derive(Debug, Default)]
enum MousePress {
    #[default]
    Idle,
    Queued(PacketKey),
    Tainted(PacketKey),
    Pressed(Option<PacketKey>, ForegroundFact),
}

#[derive(Debug)]
struct Completed {
    key: Option<PacketKey>,
    fact: ForegroundFact,
}

/// A single registration's input owner. Completed releases are immutable and
/// cannot be overwritten by another DOWN before egui renders the clicked widget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ContactKey {
    id: u64,
    location: [u64; 2],
}

#[derive(Default)]
pub(super) struct InputOwner {
    pub journal: Journal,
    mouse: MousePress,
    contacts: HashMap<u64, ForegroundFact>,
    mouse_releases: VecDeque<Completed>,
    touch_releases: HashMap<ContactKey, ForegroundFact>,
    touch_ambiguous: HashSet<ContactKey>,
    ready: Vec<Completed>,
    frame: Vec<ForegroundFact>,
    lost: bool,
}

impl InputOwner {
    pub fn retrieved(&mut self, down: Option<PacketKey>) {
        // A second retrieval before dispatch invalidates the first handoff.
        // Keep the placeholder so nested retrieval cannot supply its proof to
        // the outer message when dispatch resumes.
        if let MousePress::Queued(key) | MousePress::Tainted(key) = self.mouse {
            self.mouse = MousePress::Tainted(key);
            return;
        }
        if let Some(key) = down {
            if let MousePress::Pressed(Some(previous), _) = self.mouse {
                self.journal.retire(previous);
            }
            self.mouse = MousePress::Queued(key);
        }
    }

    pub fn mouse_down(
        &mut self,
        key: PacketKey,
        real_mouse: bool,
        promoted: Option<ForegroundFact>,
    ) {
        if let Some(fact) = promoted {
            self.mouse = MousePress::Pressed(None, fact);
            return;
        }
        if !real_mouse {
            self.cancel();
            return;
        }
        let previous = std::mem::take(&mut self.mouse);
        self.mouse = match previous {
            MousePress::Queued(queued)
                if queued.time == key.time && queued.x == key.x && queued.y == key.y =>
            {
                MousePress::Pressed(Some(key), self.journal.claim(key))
            }
            _ => {
                self.journal.retire(key);
                MousePress::Pressed(None, ForegroundFact::Unknown)
            }
        };
    }

    pub fn mouse_up(&mut self, genuine_input: bool) {
        let (key, fact) = match std::mem::take(&mut self.mouse) {
            MousePress::Pressed(key, fact) => (key, fact),
            MousePress::Queued(_) | MousePress::Tainted(_) => (None, ForegroundFact::Unknown),
            MousePress::Idle => (None, ForegroundFact::Unknown),
        };
        let fact = if genuine_input {
            fact
        } else {
            ForegroundFact::Unknown
        };
        self.mouse_releases.push_back(Completed { key, fact });
        self.check_bound();
    }

    pub fn touch_down(&mut self, id: u64, fact: ForegroundFact) {
        self.contacts.insert(id, fact);
        self.check_bound();
    }

    pub fn touch_up_at(&mut self, id: u64, location: [u64; 2]) {
        let fact = self.contacts.remove(&id).unwrap_or(ForegroundFact::Unknown);
        let key = ContactKey { id, location };
        if self.touch_ambiguous.contains(&key) || self.touch_releases.contains_key(&key) {
            self.touch_releases.remove(&key);
            self.touch_ambiguous.insert(key);
        } else {
            self.touch_releases.insert(key, fact);
        }
        self.check_bound();
    }

    #[cfg(test)]
    pub fn touch_up(&mut self, id: u64) {
        self.touch_up_at(id, [10.0_f64.to_bits(), 20.0_f64.to_bits()]);
    }

    pub fn invalidate_capture(&mut self) {
        self.lost = true;
        self.journal.invalidate();
        for fact in self
            .contacts
            .values_mut()
            .chain(self.touch_releases.values_mut())
        {
            *fact = ForegroundFact::Unknown;
        }
        for completed in self.mouse_releases.iter_mut().chain(self.ready.iter_mut()) {
            completed.fact = ForegroundFact::Unknown;
        }
    }

    pub fn cancel(&mut self) {
        match &mut self.mouse {
            MousePress::Queued(key) | MousePress::Tainted(key) => {
                self.mouse = MousePress::Tainted(*key)
            }
            MousePress::Pressed(_, fact) => *fact = ForegroundFact::Unknown,
            _ => self.mouse = MousePress::Idle,
        }
        for fact in self.contacts.values_mut() {
            *fact = ForegroundFact::Unknown;
        }
        // Already released clicks remain owned by that release, not by a
        // subsequent unrelated activation/cancellation message.
    }

    fn check_bound(&mut self) {
        if self.contacts.len()
            + self.mouse_releases.len()
            + self.touch_releases.len()
            + self.touch_ambiguous.len()
            + self.ready.len()
            > LIMIT
        {
            self.invalidate_capture();
            self.contacts.clear();
            self.mouse_releases.clear();
            self.touch_releases.clear();
            self.touch_ambiguous.clear();
            self.ready.clear();
            self.mouse = MousePress::Idle;
        }
    }

    /// Bind the native packet when its actual winit event is converted, even
    /// when the converter emitted no pointer release. Native packets for later
    /// semantic callbacks can safely remain pending across an inline ROOT frame.
    pub fn input_events(&mut self, source: eframe::NativeInputSource, events: &[egui::Event]) {
        let completed = match source {
            eframe::NativeInputSource::PrimaryMouseRelease => {
                self.mouse_releases.pop_front().unwrap_or(Completed {
                    key: None,
                    fact: ForegroundFact::Unknown,
                })
            }
            eframe::NativeInputSource::ContactEnd { id, location } => {
                let key = ContactKey { id, location };
                let fact = if self.touch_ambiguous.contains(&key) {
                    ForegroundFact::Unknown
                } else {
                    self.touch_releases
                        .remove(&key)
                        .unwrap_or(ForegroundFact::Unknown)
                };
                Completed { key: None, fact }
            }
            eframe::NativeInputSource::Other => Completed {
                key: None,
                fact: ForegroundFact::Unknown,
            },
        };
        let count = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    egui::Event::PointerButton {
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        ..
                    }
                )
            })
            .count();
        if count == 1 {
            self.ready.push(completed);
        } else {
            if let Some(key) = completed.key {
                self.journal.retire(key);
            }
            for _ in 0..count.min(LIMIT + 1) {
                self.ready.push(Completed {
                    key: None,
                    fact: ForegroundFact::Unknown,
                });
            }
        }
        self.check_bound();
    }

    /// Exactly once before ctx.run, not once per egui pass. Only producer-bound
    /// ready payloads belong to this RawInput. Pending native packets belong to
    /// their future source callback, never to an intervening frame.
    pub fn begin_input(&mut self, events: &[egui::Event]) {
        let count = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    egui::Event::PointerButton {
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        ..
                    }
                )
            })
            .count();
        let aligned = count == self.ready.len() && !self.lost;
        self.frame.clear();
        for completed in self.ready.drain(..) {
            if aligned {
                let fact = if completed
                    .key
                    .is_none_or(|key| self.journal.still_unique(key))
                {
                    completed.fact
                } else {
                    ForegroundFact::Unknown
                };
                self.frame.push(fact);
            }
            if let Some(key) = completed.key {
                self.journal.retire(key);
            }
        }
    }

    pub fn clicked(&self, release_index: Option<usize>) -> ForegroundFact {
        release_index
            .and_then(|index| self.frame.get(index))
            .cloned()
            .unwrap_or(ForegroundFact::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(time: u32) -> PacketKey {
        PacketKey {
            time,
            x: 10,
            y: 20,
            extra: 0,
        }
    }
    fn record(time: u32, window: u64) -> CaptureRecord {
        CaptureRecord {
            key: key(time),
            foreground: ForegroundFact::Window(window),
        }
    }
    fn release() -> egui::Event {
        egui::Event::PointerButton {
            pos: egui::pos2(10.0, 20.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }
    }
    fn touch_end(id: u64) -> egui::Event {
        egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(id),
            phase: egui::TouchPhase::End,
            pos: egui::pos2(10.0, 20.0),
            force: None,
        }
    }
    fn project(owner: &mut InputOwner, events: &[egui::Event]) {
        let mut index = 0;
        while index < events.len() {
            if let egui::Event::Touch {
                id,
                phase: egui::TouchPhase::End,
                ..
            } = &events[index]
            {
                let has_release = events.get(index + 1).is_some_and(|event| {
                    matches!(
                        event,
                        egui::Event::PointerButton {
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            ..
                        }
                    )
                });
                let count = if has_release { 2 } else { 1 };
                owner.input_events(
                    eframe::NativeInputSource::ContactEnd {
                        id: id.0,
                        location: [10.0_f64.to_bits(), 20.0_f64.to_bits()],
                    },
                    &events[index..index + count],
                );
                index += count;
            } else {
                let source = if matches!(
                    &events[index],
                    egui::Event::PointerButton {
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        ..
                    }
                ) {
                    eframe::NativeInputSource::PrimaryMouseRelease
                } else {
                    eframe::NativeInputSource::Other
                };
                owner.input_events(source, &events[index..index + 1]);
                index += 1;
            }
        }
        owner.begin_input(events);
    }

    fn assert_window(fact: ForegroundFact, expected: u64) {
        assert!(
            matches!(fact, ForegroundFact::Window(window) if window == expected),
            "{fact:?}"
        );
    }
    fn press(owner: &mut InputOwner, time: u32, window: u64) {
        owner.journal.insert(record(time, window));
        owner.retrieved(Some(key(time)));
        owner.mouse_down(key(time), true, None);
    }

    #[test]
    fn pre_queue_fact_survives_null_activation_and_late_activation() {
        let mut owner = InputOwner::default();
        press(&mut owner, 1, 20); // main MOUSEACTIVATE / ACTIVATE can both report NULL
        owner.mouse_up(true);
        owner.cancel(); // asynchronous inactive arrives after release
        project(&mut owner, &[release()]);
        assert_window(owner.clicked(Some(0)), 20);
    }

    #[test]
    fn completed_immutable_owner_path_controls_hide_even_after_popup_is_gone() {
        use super::super::press_capture::{ForegroundPath, ForegroundRelation};
        use crate::effetune::{GuiButtonAction, gui_button_action_for_relation};
        for (nodes, expected) in [
            (vec![(30, 200), (20, 200)], GuiButtonAction::Hide),
            (vec![(10, 100)], GuiButtonAction::Activate),
            (vec![(50, 200)], GuiButtonAction::Activate), // same host PID, unowned window
        ] {
            let mut owner = InputOwner::default();
            owner.journal.insert(CaptureRecord {
                key: key(1),
                foreground: ForegroundFact::BeforeInput(ForegroundPath::for_test(&nodes, true)),
            });
            owner.retrieved(Some(key(1)));
            owner.mouse_down(key(1), true, None);
            owner.mouse_up(true);
            project(&mut owner, &[release()]);
            let ForegroundFact::BeforeInput(path) = owner.clicked(Some(0)) else {
                panic!("missing proof")
            };
            assert_eq!(
                gui_button_action_for_relation(true, path.relation(20, 200)),
                expected
            );
            assert_eq!(
                path.relation(20, 201),
                if nodes.iter().any(|(hwnd, _)| *hwnd == 20) {
                    ForegroundRelation::Unknown
                } else {
                    ForegroundRelation::Other
                }
            );
        }
        assert_eq!(
            gui_button_action_for_relation(true, ForegroundRelation::Unknown),
            GuiButtonAction::Activate
        );
        assert_eq!(
            gui_button_action_for_relation(false, ForegroundRelation::Unknown),
            GuiButtonAction::Show
        );
    }

    #[test]
    fn release_a_then_press_b_before_render_does_not_replace_a() {
        let mut owner = InputOwner::default();
        press(&mut owner, 1, 20);
        owner.mouse_up(true);
        press(&mut owner, 2, 10);
        project(&mut owner, &[release()]);
        assert_window(owner.clicked(Some(0)), 20);
        owner.mouse_up(true);
        project(&mut owner, &[release()]);
        assert_window(owner.clicked(Some(0)), 10);
    }

    #[test]
    fn omitted_mouse_raw_release_is_consumed_without_losing_valid_later_click() {
        let mut owner = InputOwner::default();
        press(&mut owner, 1, 10);
        owner.mouse_up(true);
        // egui-winit has no pointer position, so this native release emits no RawInput.
        owner.input_events(eframe::NativeInputSource::PrimaryMouseRelease, &[]);
        press(&mut owner, 2, 20);
        owner.mouse_up(true);
        project(&mut owner, &[release()]);
        assert_window(owner.clicked(Some(0)), 20);
    }

    #[test]
    fn multiple_queued_presses_are_matched_to_their_own_releases() {
        let mut owner = InputOwner::default();
        owner.journal.insert(record(1, 20));
        owner.journal.insert(record(2, 10)); // hook thread is ahead of UI queue
        for time in [1, 2] {
            owner.retrieved(Some(key(time)));
            owner.mouse_down(key(time), true, None);
            owner.mouse_up(true);
        }
        project(&mut owner, &[release(), release()]);
        assert_window(owner.clicked(Some(0)), 20);
        assert_window(owner.clicked(Some(1)), 10);
    }

    #[test]
    fn duplicate_key_before_or_after_claim_never_uses_remaining_candidate() {
        for after_claim in [false, true] {
            let mut owner = InputOwner::default();
            owner.journal.insert(record(1, 20));
            if after_claim {
                owner.retrieved(Some(key(1)));
                owner.mouse_down(key(1), true, None);
            }
            owner.journal.insert(record(1, 10));
            if !after_claim {
                owner.retrieved(Some(key(1)));
            }
            if !after_claim {
                owner.mouse_down(key(1), true, None);
            }
            owner.mouse_up(true);
            project(&mut owner, &[release()]);
            assert!(matches!(owner.clicked(Some(0)), ForegroundFact::Unknown));
            owner.journal.insert(record(1, 20));
            assert!(matches!(
                owner.journal.claim(key(1)),
                ForegroundFact::Unknown
            ));
        }
    }

    #[test]
    fn sent_down_and_nested_retrieval_cannot_borrow_ambient_packet_proof() {
        for sent in [false, true] {
            let mut owner = InputOwner::default();
            owner.journal.insert(record(1, 20));
            owner.retrieved(Some(key(1)));
            if sent {
                owner.mouse_down(key(1), false, None);
            } else {
                owner.retrieved(None);
            }
            owner.mouse_down(key(1), true, None);
            owner.mouse_up(true);
            project(&mut owner, &[release()]);
            assert!(matches!(owner.clicked(Some(0)), ForegroundFact::Unknown));
        }
    }

    #[test]
    fn sent_release_cannot_publish_the_physical_press_fact() {
        let mut owner = InputOwner::default();
        press(&mut owner, 1, 20);
        owner.mouse_up(false);
        owner.mouse_up(true);
        project(&mut owner, &[release(), release()]);
        assert!(matches!(owner.clicked(Some(0)), ForegroundFact::Unknown));
        assert!(matches!(owner.clicked(Some(1)), ForegroundFact::Unknown));
    }

    #[test]
    fn missing_eaten_cancelled_and_retired_packets_do_not_supply_another_press() {
        let mut owner = InputOwner::default();
        owner.journal.insert(record(1, 20)); // eaten before reaching target
        owner.retrieved(Some(key(2)));
        owner.mouse_down(key(2), true, None);
        owner.mouse_up(true);
        project(&mut owner, &[release()]);
        assert!(matches!(owner.clicked(Some(0)), ForegroundFact::Unknown));
        press(&mut owner, 3, 20);
        owner.cancel();
        owner.mouse_up(true);
        project(&mut owner, &[release()]);
        assert!(matches!(owner.clicked(Some(0)), ForegroundFact::Unknown));
        project(&mut owner, &[]); // hidden toolbar still consumes frame boundary
        assert!(matches!(owner.clicked(Some(0)), ForegroundFact::Unknown));
        let fresh = InputOwner::default();
        assert!(matches!(fresh.clicked(Some(0)), ForegroundFact::Unknown));
    }

    #[test]
    fn touch_ids_and_secondary_contact_selection_do_not_depend_on_primary_flag() {
        let mut owner = InputOwner::default();
        owner.touch_down(1, ForegroundFact::Window(20));
        owner.touch_down(2, ForegroundFact::Window(10));
        owner.touch_up(2); // ignored by egui while 1 is selected
        owner.touch_up(1);
        owner.touch_down(3, ForegroundFact::Window(20)); // now selected, no PRIMARY flag
        owner.touch_up(3);
        project(
            &mut owner,
            &[
                touch_end(2),
                touch_end(1),
                release(),
                touch_end(3),
                release(),
            ],
        );
        assert_window(owner.clicked(Some(0)), 20);
        assert_window(owner.clicked(Some(1)), 20);
    }

    #[test]
    fn missing_mouse_stream_does_not_invalidate_known_touch_or_balance_counts() {
        let mut owner = InputOwner::default();
        owner.touch_down(1, ForegroundFact::Window(20));
        owner.touch_up(1);
        // Equal total counts but wrong sources is not a matching proof.
        project(&mut owner, &[release(), touch_end(1), release()]);
        assert!(matches!(owner.clicked(Some(0)), ForegroundFact::Unknown));
        assert_window(owner.clicked(Some(1)), 20);
    }

    #[test]
    fn omitted_touch_end_and_reused_contact_cannot_shift_earlier_fact() {
        let mut owner = InputOwner::default();
        owner.touch_down(1, ForegroundFact::Window(20));
        owner.touch_up(1);
        owner.touch_down(1, ForegroundFact::Window(10));
        owner.touch_up(1);
        owner.touch_down(2, ForegroundFact::Window(20));
        owner.touch_up(2);
        project(
            &mut owner,
            &[touch_end(1), release(), touch_end(2), release()],
        );
        assert!(matches!(owner.clicked(Some(0)), ForegroundFact::Unknown));
        assert_window(owner.clicked(Some(1)), 20);
        project(&mut owner, &[touch_end(1), release()]);
        assert!(matches!(owner.clicked(Some(0)), ForegroundFact::Unknown));
    }

    #[test]
    fn promoted_mouse_uses_pointer_fact_but_does_not_merge_release_sources() {
        let mut owner = InputOwner::default();
        owner.mouse_down(key(1), false, Some(ForegroundFact::Window(20)));
        owner.mouse_up(true);
        project(&mut owner, &[release()]);
        assert_window(owner.clicked(Some(0)), 20);
    }

    #[test]
    fn overflow_is_terminal_unknown_not_a_reset_that_can_reuse_old_keys() {
        let mut owner = InputOwner::default();
        for time in 0..=LIMIT as u32 {
            owner.journal.insert(record(time, 20));
        }
        owner.journal.insert(record(1000, 20));
        owner.retrieved(Some(key(1000)));
        owner.mouse_down(key(1000), true, None);
        owner.mouse_up(true);
        project(&mut owner, &[release()]);
        assert!(matches!(owner.clicked(Some(0)), ForegroundFact::Unknown));
    }

    #[test]
    fn buffered_mouse_completion_survives_an_inline_frame_before_its_source_callback() {
        let mut owner = InputOwner::default();
        for (time, window) in [(1, 20), (2, 10)] {
            press(&mut owner, time, window);
            owner.mouse_up(true);
        }
        // winit may buffer both callbacks, with an inline resize frame between them.
        project(&mut owner, &[release()]);
        assert_window(owner.clicked(Some(0)), 20);
        project(&mut owner, &[]);
        project(&mut owner, &[release()]);
        assert_window(owner.clicked(Some(0)), 10);
    }

    #[test]
    fn buffered_contact_completion_survives_an_inline_frame_before_its_source_callback() {
        let mut owner = InputOwner::default();
        for (id, window) in [(1, 20), (2, 10)] {
            owner.touch_down(id, ForegroundFact::Window(window));
            owner.touch_up(id);
        }
        project(&mut owner, &[touch_end(1), release()]);
        assert_window(owner.clicked(Some(0)), 20);
        project(&mut owner, &[]);
        project(&mut owner, &[touch_end(2), release()]);
        assert_window(owner.clicked(Some(0)), 10);
    }

    #[test]
    fn filtered_contact_and_reused_id_at_another_location_keep_distinct_facts() {
        let mut owner = InputOwner::default();
        let location_a = [1.25_f64.to_bits(), 2.5_f64.to_bits()];
        let location_b = [10.0_f64.to_bits(), 20.0_f64.to_bits()];
        owner.touch_down(1, ForegroundFact::Window(20));
        owner.touch_up_at(1, location_a);
        // No semantic End was produced for A; winit subsequently reuses the ID.
        project(&mut owner, &[]);
        owner.touch_down(1, ForegroundFact::Window(10));
        owner.touch_up_at(1, location_b);
        project(&mut owner, &[touch_end(1), release()]);
        assert_window(owner.clicked(Some(0)), 10);
        owner.input_events(
            eframe::NativeInputSource::ContactEnd {
                id: 1,
                location: location_a,
            },
            &[release()],
        );
        owner.begin_input(&[release()]);
        assert_window(owner.clicked(Some(0)), 20);
    }

    #[test]
    fn failed_contact_capture_cannot_reuse_an_older_pending_or_ready_fact() {
        for bind_before_failure in [false, true] {
            let mut owner = InputOwner::default();
            owner.touch_down(1, ForegroundFact::Window(20));
            owner.touch_up(1);
            if bind_before_failure {
                owner.input_events(
                    eframe::NativeInputSource::ContactEnd {
                        id: 1,
                        location: [10.0_f64.to_bits(), 20.0_f64.to_bits()],
                    },
                    &[release()],
                );
            }
            // Native history/coordinate read failed, while winit may still emit B.
            owner.invalidate_capture();
            if bind_before_failure {
                owner.begin_input(&[release()]);
            } else {
                project(&mut owner, &[touch_end(1), release()]);
            }
            assert!(matches!(owner.clicked(Some(0)), ForegroundFact::Unknown));
        }
    }

    #[test]
    fn empty_contact_suffix_consumes_only_its_own_completion() {
        let mut owner = InputOwner::default();
        for (id, window) in [(1, 20), (2, 10)] {
            owner.touch_down(id, ForegroundFact::Window(window));
            owner.touch_up(id);
        }
        owner.input_events(
            eframe::NativeInputSource::ContactEnd {
                id: 1,
                location: [10.0_f64.to_bits(), 20.0_f64.to_bits()],
            },
            &[],
        );
        project(&mut owner, &[]);
        project(&mut owner, &[touch_end(2), release()]);
        assert_window(owner.clicked(Some(0)), 10);
        project(&mut owner, &[touch_end(1), release()]);
        assert!(matches!(owner.clicked(Some(0)), ForegroundFact::Unknown));
    }
}
