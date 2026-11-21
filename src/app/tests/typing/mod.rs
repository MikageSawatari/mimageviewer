//! §1.832: real App frames, one character at a time. Never copy App's phase order.
#![cfg(windows)]

use super::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use crate::ime_focus::typing_probe::{self as probe, Outcome};
use std::time::{Duration, Instant};

mod dialogs;
mod facets;
mod inventory;
mod search;

const DEADLINE: Duration = Duration::from_secs(10);

// Shared runner measurement (2026-11-21), relative to the product open frame 0.
// Each constant is max(first drawn, first enabled, first focus, first cursor).
// Temporary Local/Container/Metadata: [0,0,0,1]; pinned: [0,0,1,2].
// Pinned entry hands its focus reservation/latch to the drawn field through
// src/app/pinned_search.rs:1100,1488 and src/ui_pinned_search.rs:207.
// crates/egui-ime/src/lib.rs:522-528 applies that focus AFTER TextEdit::show
// (:465 requests it); vendor/egui/src/widgets/text_edit/builder.rs:656,1519
// initializes/persists the cursor in a focused draw. The measured pinned focus
// handoff is frame 1, followed by its cursor in frame 2: one frame after the
// temporary bar, not a field that must be allowed to lose focus while typing.
const TEMPORARY_SEARCH_OPENING_FRAME: u64 = 1;
const PINNED_SEARCH_OPENING_FRAME: u64 = 2;
// Everything explicitly stores a selection at entry, including an empty one:
// src/ui_pinned_search.rs:245-256; src/global_search_ui/everything/ui.rs:110.
// Measured temporary [0,0,0,0], pinned [0,0,1,1].
const EVERYTHING_TEMPORARY_OPENING_FRAME: u64 = 0;
const EVERYTHING_PINNED_OPENING_FRAME: u64 = 1;
// Ctrl+T [0,0,0,1]; address/history [0,0,2,2]; tag suggestions [0,1,0,1];
// collection creation and both smart-folder rule editors [0,1,2,2].
// Clicked entries move the pointer in frame 1 and press in frame 2. Readiness
// precedes release/path clearing; those input operations are checked separately.
const TAG_VIEW_OPENING_FRAME: u64 = 1;
const ADDRESS_OPENING_FRAME: u64 = 2;
const TAG_SUGGESTIONS_OPENING_FRAME: u64 = 1;
const DIALOG_OPENING_FRAME: u64 = 2;
const MEASUREMENT_LAST_FRAME: u64 = 9;

#[derive(Clone, Copy)]
enum FieldChecks {
    Opening,
    /// All four readiness conditions, including during release/path clearing.
    Ready,
    /// Also the draft, exact cursor endpoints, IME owner and adapter invariants.
    Typing,
}

#[derive(Debug)]
struct OpeningSample {
    relative_frame: u64,
    backend_frame: u64,
    // Drawn, enabled (every draw in this update), has_focus, saved cursor range.
    ready: [bool; 4],
    focused: Option<egui::Id>,
    input_focused: bool,
    fields: Vec<probe::Field>,
    focus_events: Vec<probe::FocusEvent>,
}

#[derive(Clone, Copy)]
enum Locator {
    Id(egui::Id),
    /// Auto-generated dialog ids are observed at their real helper call site.
    Site(&'static str, &'static str),
}

impl Locator {
    fn resolve(self) -> Option<egui::Id> {
        match self {
            Self::Id(id) => Some(id),
            Self::Site(path, anchor) => {
                let source =
                    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
                        .expect("read field source");
                let lines: Vec<_> = source.lines().collect();
                let mut ids: Vec<_> = probe::fields()
                    .into_iter()
                    .filter(|field| {
                        field.file.replace('\\', "/").ends_with(path)
                            && lines
                                .iter()
                                .skip(field.line as usize - 1)
                                .take(6)
                                .any(|line| line.contains(anchor))
                    })
                    .map(|field| field.id)
                    .collect();
                ids.sort_by_key(|id| id.value());
                ids.dedup();
                assert!(
                    ids.len() <= 1,
                    "ambiguous typing field: {path} / {anchor}: {ids:?}"
                );
                ids.first().copied()
            }
        }
    }
}

trait Adapter {
    fn label(&self) -> String;
    fn open(&self, app: &mut App, ctx: &egui::Context);
    /// Search admission must run inside the actual keyboard handler's pass.
    /// Dialogs without a shortcut retain their normal product open function.
    fn opening_shortcut(&self) -> Option<(egui::Key, crate::keymap::KeyAction)> {
        None
    }
    fn locator(&self) -> Locator;
    fn draft<'a>(&self, app: &'a App) -> &'a str;
    /// Search entries promise focus themselves; ordinary dialog/address fields are clicked.
    fn opens_focused(&self) -> bool {
        true
    }
    fn opening_frame(&self) -> u64;
    fn opening_button(&self) -> Option<&'static str> {
        None
    }
    /// Expected product policy, independent of the observed debounce state.
    fn defers_results_during_ime(&self) -> bool {
        false
    }
    /// Observed state only. None is a violation when IME deferral is expected.
    fn ime_deferred_deadline(&self, _app: &App) -> Option<Instant> {
        None
    }
    fn check_ime_deferred(&self, _app: &App, _query: &str) {}
    fn job(&self) -> Option<&'static str> {
        None
    }
    fn outcome(&self, _query: &str) -> Outcome {
        unreachable!("no per-edit producer")
    }
    fn adopted(&self, _app: &App, _query: &str, _outcome: Outcome) -> bool {
        true
    }
    fn check_idle(&self, _app: &App) {}
}

fn fixture() -> AppTestEnv {
    let mut env = setup_app();
    env.initialized = true;
    env.indexer_init = IndexerInit::Unavailable;
    env.settings.update_check_enabled = false;
    env.settings.minimize_to_tray_on_close = false;
    env.settings.show_toolbar_search_local = false;
    env.settings.show_toolbar_search_container = false;
    env.settings.show_toolbar_search_metadata = false;
    env.settings.show_toolbar_search_everything = false;
    env.last_main_focused = true;
    env
}

struct Harness<A> {
    // The common fixture owns the data-dir override, workers, and (after §1.814g)
    // the native input lease. This harness must never acquire a second lease.
    env: AppTestEnv,
    adapter: A,
    backend: eframe::HeadlessEpiIntegration,
    ctx: egui::Context,
    id: egui::Id,
    expected: String,
    composition_start: Option<usize>,
    stage: String,
    frame: u64,
    opening_start: Option<u64>,
    opening_samples: Vec<OpeningSample>,
    checks: FieldChecks,
    _observations: probe::Session,
}

impl<A: Adapter> Harness<A> {
    fn new_with_measurement(
        env: AppTestEnv,
        adapter: A,
        origin_paths: Vec<PathBuf>,
        measure: bool,
    ) -> Self {
        assert!(
            !origin_paths.is_empty(),
            "typing fixture needs an observable physical origin"
        );
        let observations = probe::Session::new();
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        crate::ime_focus::install_ime_input_policy(&ctx);
        let backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
        let mut harness = Self {
            env,
            adapter,
            backend,
            ctx,
            id: egui::Id::NULL,
            expected: String::new(),
            composition_start: None,
            stage: "adopt physical origin before opening".into(),
            frame: 0,
            opening_start: None,
            opening_samples: Vec::new(),
            checks: FieldChecks::Opening,
            _observations: observations,
        };
        // seed_images starts the product folder load. A vector assigned to
        // App.items is not a published physical listing: it lacks its identity /
        // hydration / navigation owners. Wait for that source's actual adoption,
        // not for a search field to recover focus or become enabled afterwards.
        let deadline = Instant::now() + DEADLINE;
        loop {
            harness.tick(Vec::new());
            if harness.env.items.len() == origin_paths.len()
                && harness.env.items.iter().all(|item| {
                    item.drag_source_path().is_some_and(|path| {
                        origin_paths
                            .iter()
                            .any(|expected| expected.as_path() == path)
                    })
                })
                && harness.env.current_folder.as_deref() == origin_paths[0].parent()
                && !harness.env.main_chrome_pointer_blocked()
                && !harness.env.sidecar_restore_active()
                && harness.env.folder_pane_open_pending.is_none()
                && harness.env.external_rescan_pending.is_none()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "physical origin did not adopt {origin_paths:?}: {}",
                harness.diagnostic()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let opening_pointer = harness.adapter.opening_button().map(|name| {
            let center = probe::target_rect(name)
                .expect("enabled opening button")
                .center();
            harness.tick(vec![egui::Event::PointerMoved(center)]);
            harness.tick(vec![pointer(center, true)]);
            center
        });
        harness.stage = "open field".into();
        harness.opening_start = Some(harness.frame + 1);
        let shortcut = harness.adapter.opening_shortcut();
        if let Some((code, action)) = shortcut {
            let ctrl = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
            harness.tick_with_open_action(vec![key(code, true, ctrl)], Some(action));
        } else {
            harness.adapter.open(&mut harness.env, &harness.ctx);
            harness.tick(
                opening_pointer
                    .map(|center| vec![pointer(center, false)])
                    .unwrap_or_default(),
            );
        }
        // Keep the actual input sequence identical in measurement and typing
        // runs. Its release/clear frames do not define field readiness.
        let mut setup_events = Vec::new();
        if let Some((code, _)) = shortcut {
            setup_events.push(vec![key(
                code,
                false,
                egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            )]);
        }
        if !harness.adapter.opens_focused() {
            let center = probe::fields()
                .iter()
                .rev()
                .find(|field| field.id == harness.id)
                .expect("current field response")
                .rect
                .center();
            setup_events.push(vec![egui::Event::PointerMoved(center)]);
            setup_events.push(vec![pointer(center, true)]);
            setup_events.push(vec![pointer(center, false)]);
        }
        // Address fields may initially show the current path. Clear through TextEdit,
        // without overwriting App's draft or changing its cursor/focus state.
        if !harness.adapter.draft(&harness.env).is_empty() {
            let ctrl = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
            setup_events.push(vec![key(egui::Key::A, true, ctrl)]);
            setup_events.push(vec![key(egui::Key::A, false, ctrl)]);
            setup_events.push(vec![key(egui::Key::Backspace, true, egui::Modifiers::NONE)]);
            setup_events.push(vec![key(
                egui::Key::Backspace,
                false,
                egui::Modifiers::NONE,
            )]);
        }
        let ready_frame = harness.adapter.opening_frame();
        let last_frame = if measure {
            // Fixed observation window, independent of the readiness constant:
            // moving a constant must not move the measured focus/cursor events.
            assert!(setup_events.len() as u64 <= MEASUREMENT_LAST_FRAME);
            MEASUREMENT_LAST_FRAME
        } else {
            ready_frame.max(setup_events.len() as u64)
        };
        if !measure && ready_frame == 0 {
            let _ = harness.assert_opening_ready();
            harness.checks = FieldChecks::Ready;
        }
        let mut setup_events = setup_events.into_iter();
        for relative_frame in 1..=last_frame {
            harness.tick(setup_events.next().unwrap_or_default());
            if !measure && relative_frame == ready_frame {
                let _ = harness.assert_opening_ready();
                harness.checks = FieldChecks::Ready;
            }
        }
        if measure {
            // Print every field before the diagnostic test asserts its findings.
            harness.print_opening_measurement();
        } else {
            harness.stage = "open empty field".into();
            harness.assert_field();
            harness.checks = FieldChecks::Typing;
        }
        harness.opening_start = None;
        harness
    }

    fn tick(&mut self, events: Vec<egui::Event>) {
        self.tick_with_open_action(events, None);
    }

    fn tick_with_open_action(
        &mut self,
        events: Vec<egui::Event>,
        opening: Option<crate::keymap::KeyAction>,
    ) {
        self.frame += 1;
        probe::begin_frame();
        let modifiers = events
            .iter()
            .find_map(|event| match event {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or(egui::Modifiers::NONE);
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 900.0),
            )),
            focused: true,
            events: std::iter::once(egui::Event::ModifiersChanged(modifiers))
                .chain(events)
                .collect(),
            ..Default::default()
        };
        raw.viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .focused = Some(true);
        // EpiIntegration overwrites RawInput.time with its elapsed real time.
        // Metadata's debounce uses Instant as well; do not fast-forward either clock.
        self.backend
            .update(&mut self.env.app, raw)
            .drop_without_applying_deltas();
        let actions = probe::take_actions();
        assert!(
            match opening {
                Some(expected) =>
                    !actions.is_empty() && actions.iter().all(|(action, _)| *action == expected),
                None => actions.is_empty(),
            },
            "typed input became KeyAction {actions:?}: {}",
            self.diagnostic()
        );
        if let Some(start) = self.opening_start {
            if let Some(id) = self.adapter.locator().resolve() {
                self.id = id;
            }
            let fields: Vec<_> = probe::fields()
                .into_iter()
                .filter(|field| field.id == self.id)
                .collect();
            let drawn = !fields.is_empty();
            self.opening_samples.push(OpeningSample {
                relative_frame: self.frame - start,
                backend_frame: self.frame,
                ready: [
                    drawn,
                    drawn && fields.iter().all(|field| field.enabled),
                    self.ctx.memory(|memory| memory.has_focus(self.id)),
                    egui::TextEdit::load_state(&self.ctx, self.id)
                        .is_some_and(|state| state.cursor.char_range().is_some()),
                ],
                focused: self.ctx.memory(|memory| memory.focused()),
                input_focused: self.ctx.input(|input| input.focused),
                fields,
                focus_events: probe::focus_events(),
            });
        }
        match self.checks {
            FieldChecks::Opening => {}
            FieldChecks::Ready => {
                let _ = self.assert_opening_ready();
            }
            FieldChecks::Typing => self.assert_field(),
        }
    }

    fn opening_first_frames(&self) -> [Option<u64>; 4] {
        std::array::from_fn(|index| {
            self.opening_samples
                .iter()
                .find(|sample| sample.ready[index])
                .map(|sample| sample.relative_frame)
        })
    }

    fn first_all_ready_frame(&self) -> Option<u64> {
        self.opening_samples
            .iter()
            .find(|sample| sample.ready.into_iter().all(|ready| ready))
            .map(|sample| sample.relative_frame)
    }

    fn print_opening_measurement(&self) {
        let first = self.opening_first_frames();
        eprintln!(
            "typing opening {}: first [drawn, enabled, focus, cursor]={first:?}; first_all={:?}; contract={}",
            self.adapter.label(),
            self.first_all_ready_frame(),
            self.adapter.opening_frame()
        );
        for sample in &self.opening_samples {
            eprintln!("  {sample:?}");
        }
    }

    fn measured_opening_contract_failure(&self) -> Option<String> {
        let expected = self.adapter.opening_frame();
        let measured = self.first_all_ready_frame();
        let first = self.opening_first_frames();
        let latest_first = first
            .into_iter()
            .try_fold(0_u64, |latest, frame| frame.map(|frame| latest.max(frame)));
        let dropped: Vec<_> = self
            .opening_samples
            .iter()
            .filter(|sample| {
                sample.relative_frame >= expected && !sample.ready.into_iter().all(|ready| ready)
            })
            .map(|sample| (sample.relative_frame, sample.ready))
            .collect();
        let measured_to_end = self
            .opening_samples
            .last()
            .is_some_and(|sample| sample.relative_frame == MEASUREMENT_LAST_FRAME);
        (measured != Some(expected) || latest_first != Some(expected)
            || !dropped.is_empty() || !measured_to_end).then(|| format!(
            "{}: contract={expected}, measured first_all={measured:?}, max_first={latest_first:?}, first conditions={first:?}, dropped={dropped:?}, measured_to_frame_9={measured_to_end}",
            self.adapter.label(),
        ))
    }

    fn diagnostic(&self) -> String {
        format!(
            "{} stage={} chars={} frame={} expected={:?} draft={:?} focus={:?} field={:?} local_pending={} container={:?}/pending={}/results={} tag={:?}/pending={}/results={} metadata={:?}/pending={}/done={}/reject={:?}/items={} hydration={} modal={:?} fields={:?} gates={:?} jobs={:?} focus_events={:?} opening={:?}",
            self.adapter.label(),
            self.stage,
            self.expected.chars().count(),
            self.frame,
            self.expected,
            self.adapter.draft(&self.env),
            self.ctx.memory(|m| m.focused()),
            self.id,
            self.env.search_pending.is_some(),
            self.env.favsearch.last_executed,
            self.env.favsearch_pending.is_some(),
            self.env.favsearch.results_paths.len(),
            self.env.tag_view.last_executed,
            self.env.tag_view_pending.is_some(),
            self.env.tag_view.result_count,
            self.env.global_search.last_executed,
            self.env.global_search.pending.is_some(),
            self.env.global_search.done,
            self.env.global_search.reject_message,
            self.env.items.len(),
            self.env.sidecar_restore_active(),
            self.env.modal_dialog_block_reason(),
            probe::fields()
                .iter()
                .map(|field| (
                    field.id,
                    field.frame,
                    field.pass,
                    field.enabled,
                    field.focused_when_drawn
                ))
                .collect::<Vec<_>>(),
            probe::gates()
                .iter()
                .map(|gate| (gate.pass, gate.reason))
                .collect::<Vec<_>>(),
            probe::jobs(),
            probe::focus_events(),
            self.opening_samples,
        )
    }

    fn assert_opening_ready(&self) -> egui::text::CCursorRange {
        let diagnostic = self.diagnostic();
        assert!(
            self.ctx.memory(|m| m.has_focus(self.id)),
            "lost caret: {diagnostic}"
        );
        assert!(
            probe::fields().iter().any(|field| field.id == self.id),
            "field not drawn in this frame: {diagnostic}"
        );
        // Context::end_pass swaps this_pass/prev_pass. read_response prefers
        // this_pass and is not a current-draw witness outside an active pass.
        // Inspect every actual helper response in this backend frame, including
        // a disabled intermediate pass; a later pass must not hide a bad draw.
        assert!(
            probe::fields()
                .iter()
                .filter(|field| field.id == self.id)
                .all(|field| field.enabled),
            "disabled field: {diagnostic}"
        );
        egui::TextEdit::load_state(&self.ctx, self.id)
            .and_then(|state| state.cursor.char_range())
            .unwrap_or_else(|| panic!("missing TextEdit cursor: {diagnostic}"))
    }

    fn assert_field(&self) {
        let range = self.assert_opening_ready();
        let diagnostic = self.diagnostic();
        assert_eq!(
            self.adapter.draft(&self.env),
            self.expected,
            "draft changed: {diagnostic}"
        );
        {
            let sorted = range.as_sorted_char_range();
            let end = self.expected.chars().count();
            assert_eq!(sorted.end.0, end, "caret/composition end: {diagnostic}");
            assert_eq!(
                sorted.start.0,
                self.composition_start.unwrap_or(end),
                "TextEdit selection/composition range: {diagnostic}"
            );
        }
        assert!(
            self.ctx.memory(|m| m.owns_ime_events(self.id)),
            "IME events no longer belong to field: {diagnostic}"
        );
        // egui 0.37's cursor_purpose is private. Its persisted composition selection,
        // the draft including preedit, and the shared IME owner are public witnesses.
        assert_eq!(
            crate::ime_focus::ime_input_active(&self.ctx),
            self.composition_start.is_some(),
            "composition owner: {diagnostic}"
        );
        self.adapter.check_idle(&self.env);
    }

    fn result_progress(&self, since: usize) -> (bool, bool) {
        let Some(field) = self.adapter.job() else {
            return (true, true);
        };
        let expected = self.adapter.outcome(&self.expected);
        let jobs = probe::jobs();
        let relevant: Vec<_> = jobs
            .iter()
            .skip(since)
            .filter(|job| job.field == field && job.query == self.expected)
            .collect();
        let started = relevant
            .iter()
            .position(|job| job.outcome == Outcome::Started);
        let completed = started.is_some_and(|start| {
            relevant
                .iter()
                .skip(start + 1)
                .any(|job| job.outcome == expected)
        });
        (
            started.is_some(),
            completed && self.adapter.adopted(&self.env, &self.expected, expected),
        )
    }

    fn assert_ime_deferral(&self) -> Instant {
        let deadline = self
            .adapter
            .ime_deferred_deadline(&self.env)
            .unwrap_or_else(|| panic!("missing IME deferral deadline: {}", self.diagnostic()));
        self.adapter.check_ime_deferred(&self.env, &self.expected);
        deadline
    }

    fn wait_result(&mut self, since: usize) {
        if self.composition_start.is_some() && self.adapter.defers_results_during_ime() {
            // Policy selects this branch. Losing the deadline, including after
            // an erroneous early adoption, must never opt out of the check.
            let original_deadline = self.assert_ime_deferral();
            // Exercise the product's actual 150 ms boundary while composing.
            // Elapsing it is not a result: the deadline must be renewed, the
            // old applied query retained, and no delivery may have occurred.
            let timeout = Instant::now() + DEADLINE;
            loop {
                self.assert_field();
                let _ = self.assert_ime_deferral();
                let (started, complete) = self.result_progress(since);
                assert!(
                    started && !complete,
                    "IME deferred producer: {}",
                    self.diagnostic()
                );
                if Instant::now() >= original_deadline {
                    // Observe a real App update at/after the expired boundary,
                    // even if the runner was descheduled between earlier ticks.
                    self.tick(Vec::new());
                    let renewed_deadline = self.assert_ime_deferral();
                    assert!(
                        renewed_deadline > original_deadline,
                        "IME did not renew debounce: {}",
                        self.diagnostic()
                    );
                    return;
                }
                assert!(
                    Instant::now() < timeout,
                    "IME debounce deadline: {}",
                    self.diagnostic()
                );
                std::thread::sleep(Duration::from_millis(1));
                self.tick(Vec::new());
            }
        }
        let Some(field) = self.adapter.job() else {
            self.assert_field();
            return;
        };
        let expected = self.adapter.outcome(&self.expected);
        let deadline = Instant::now() + DEADLINE;
        loop {
            self.assert_field();
            let (started, complete) = self.result_progress(since);
            if complete {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "result deadline ({field}, wanted {expected:?}, started={started:?}): {}",
                self.diagnostic()
            );
            // Pacing only: neither elapsed time nor a frame count means success.
            std::thread::sleep(Duration::from_millis(1));
            self.tick(Vec::new());
        }
    }

    pub(super) fn type_text(&mut self, text: &str) {
        for (index, character) in text.chars().enumerate() {
            self.stage = format!("type {character:?}, character {}", index + 1);
            let since = probe::jobs().len();
            self.expected.push(character);
            let letter_key = character.is_ascii_alphabetic().then(|| {
                egui::Key::from_name(&character.to_ascii_uppercase().to_string())
                    .expect("egui letter key")
            });
            let mut events = Vec::new();
            if let Some(key_code) = letter_key {
                events.push(key(key_code, true, egui::Modifiers::NONE));
            }
            events.push(egui::Event::Text(character.to_string()));
            self.tick(events);
            self.assert_field();
            // winit delivers a separate release frame. Non-ASCII Text has no Key.
            self.tick(
                letter_key
                    .map(|code| vec![key(code, false, egui::Modifiers::NONE)])
                    .unwrap_or_default(),
            );
            self.wait_result(since);
        }
    }

    #[expect(
        deprecated,
        reason = "Windows winit still delivers Enabled/Disabled around composition"
    )]
    pub(super) fn compose_ime(&mut self, steps: &[&str], commit: &str) {
        assert!(!steps.is_empty());
        let base = self.expected.clone();
        self.stage = "IME Enabled".into();
        // Enabled alone is not a nonempty preedit; the shared policy owns it already.
        self.composition_start = Some(base.chars().count());
        self.tick(vec![egui::Event::Ime(egui::ImeEvent::Enabled)]);
        self.assert_field();
        for (index, preedit) in steps.iter().enumerate() {
            self.stage = format!("IME Preedit {preedit:?}, step {}", index + 1);
            let since = probe::jobs().len();
            let changed = self.expected != format!("{base}{preedit}");
            self.expected = format!("{base}{preedit}");
            self.tick(vec![egui::Event::Ime(egui::ImeEvent::Preedit {
                text: (*preedit).into(),
                active_range_chars: None,
            })]);
            self.assert_field();
            self.wait_result(if changed { since } else { 0 });
        }
        self.stage = format!("IME Commit {commit:?}");
        let since = probe::jobs().len();
        let changed = self.expected != format!("{base}{commit}");
        self.expected = format!("{base}{commit}");
        self.composition_start = None;
        self.tick(vec![egui::Event::Ime(egui::ImeEvent::Commit(
            commit.into(),
        ))]);
        self.assert_field();
        // The last preedit's completed result remains valid if Commit has the same
        // text. Do not require a fictitious second worker for an unchanged draft.
        self.wait_result(if changed { since } else { 0 });
        self.stage = "IME Disabled after Commit".into();
        self.tick(vec![egui::Event::Ime(egui::ImeEvent::Disabled)]);
        self.assert_field();
    }
}

fn key(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    }
}

/// Run with nextest --no-capture to obtain all 16 opening measurements in one
/// process. Capture every mismatch before failing, including earlier readiness
/// and any loss of readiness through frame 9.
#[test]
fn typing_opening_readiness_measurement() {
    let mut failures = Vec::new();
    search::measure_openings(&mut failures);
    dialogs::measure_openings(&mut failures);
    facets::measure_openings(&mut failures);
    assert!(
        failures.is_empty(),
        "opening readiness changed (all measurements printed above): {failures:?}"
    );
}

fn pointer(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        pressed,
        button: egui::PointerButton::Primary,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn typing_fixture_letters_include_default_single_key_actions() {
    use crate::keymap::{Chord, KeyAction, KeyName};
    for (action, name) in [
        (KeyAction::GridRotateCcw, KeyName::L),
        (KeyAction::GridPin, KeyName::P),
        (KeyAction::GridRotateCw, KeyName::R),
    ] {
        assert!(
            action
                .default_chords()
                .iter()
                .any(|chord| chord == Chord::key(name))
        );
    }
    let defaults = include_str!("../../../../docs/keymap.ini.default");
    for binding in ["GridRotateCcw", "GridPin", "GridRotateCw"] {
        assert!(defaults.contains(binding));
    }
}

#[test]
fn typing_shortcut_probe_records_successful_keymap_decisions() {
    use crate::keymap::{KeyAction, Keymap};
    let _env = fixture(); // Own the same input lease as App-driven cases.
    let _observations = probe::Session::new();
    let ctx = egui::Context::default();
    let keymap = Keymap::default();
    let raw = egui::RawInput {
        events: vec![key(egui::Key::L, true, egui::Modifiers::NONE)],
        ..Default::default()
    };
    ctx.run_ui(raw, |_| {
        crate::keyboard_input::cache_keyboard_owner(
            &ctx,
            crate::keyboard_input::KeyboardOwner::Unclaimed,
        );
        assert!(!keymap.pressed_action(&ctx, KeyAction::GridPin));
        assert!(probe::take_actions().is_empty());
        assert!(keymap.pressed_action(&ctx, KeyAction::GridRotateCcw));
        assert!(keymap.consume_action_no_repeat(&ctx, KeyAction::GridRotateCcw));
    })
    .drop_without_applying_deltas();
    let observed = probe::take_actions();
    assert_eq!(
        observed.len(),
        2,
        "positive decisions must be recorded even with key-debug disabled: {observed:?}"
    );
    assert!(
        observed
            .iter()
            .all(|(action, _)| *action == KeyAction::GridRotateCcw)
    );
}

#[test]
fn typing_response_observation_uses_the_drawn_pass() {
    let _observations = probe::Session::new();
    let ctx = egui::Context::default();
    let id = egui::Id::unique("typing-response-observation");
    for enabled in [false, true] {
        probe::begin_frame();
        ctx.run_ui(Default::default(), |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                let response = ui.interact(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(80.0, 24.0)),
                    id,
                    egui::Sense::click(),
                );
                probe::field(&response, std::panic::Location::caller());
            });
        })
        .drop_without_applying_deltas();
        let fields = probe::fields();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].enabled, enabled);
    }
    // After end_pass, egui 0.37's this_pass holds the preceding pass's
    // widgets, and read_response prefers those over the latest prev_pass.
    // The actual second draw above was enabled; an out-of-pass read is stale.
    assert!(!ctx.read_response(id).unwrap().enabled());
    assert!(probe::fields()[0].enabled);
}
