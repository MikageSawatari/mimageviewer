//! Handler regressions through the host policy and real editor widgets.
use super::*;
use egui::{Event, Key, Modifiers, Pos2, Rect, TextBuffer, Vec2};
use musicvideo_core::{BandRange, ModulatorSource, default_project, new_band_energy_modulator};

#[derive(Clone, Copy, Debug)]
enum Field {
    LayerName,
    Text,
    ModulatorName,
    LowNote,
    HighNote,
    Lyrics,
}

const SINGLELINES: [Field; 5] = [
    Field::LayerName,
    Field::Text,
    Field::ModulatorName,
    Field::LowNote,
    Field::HighNote,
];

impl Field {
    fn initial_text(self) -> &'static str {
        match self {
            Self::LayerName => "ime-layer",
            Self::Text => "ime-body",
            Self::ModulatorName => "ime-modulator",
            Self::LowNote => "C1",
            Self::HighNote => "C4",
            Self::Lyrics => "original\n\nsecond",
        }
    }

    fn tab(self) -> InspectorTab {
        match self {
            Self::ModulatorName | Self::LowNote | Self::HighNote => InspectorTab::Modulators,
            _ => InspectorTab::Layer,
        }
    }
}

struct Harness {
    context: egui::Context,
    project: Project,
    state: EditorState,
    layer: LayerId,
    field: Field,
    time: f64,
    output: egui::FullOutput,
    inspector: Rect,
    response: EditorResponse,
    events_after_shortcut: Vec<Event>,
    recovery_before_shortcut: Option<egui::Id>,
    wants_keyboard_before_shortcut: bool,
}

impl Harness {
    fn new(field: Field) -> Self {
        let mut project = default_project(10.0);
        let mut layer = if matches!(field, Field::Lyrics) {
            let mut layer = new_lyrics_layer(&project);
            let LayerKind::Lyrics(lyrics) = &mut layer.kind else {
                unreachable!()
            };
            lyrics.lines = vec![
                LyricLine {
                    text: "original".into(),
                    time_secs: Some(3.0),
                },
                LyricLine {
                    text: String::new(),
                    time_secs: None,
                },
                LyricLine {
                    text: "second".into(),
                    time_secs: Some(6.0),
                },
            ];
            layer
        } else {
            let mut layer = new_text_layer(&project);
            let LayerKind::Text(text) = &mut layer.kind else {
                unreachable!()
            };
            text.text = Field::Text.initial_text().into();
            layer
        };
        layer.name = Field::LayerName.initial_text().into();
        let layer_id = layer.id;
        project.layers.push(layer);
        project.modulators.clear();
        let mut modulator = new_band_energy_modulator(&project);
        modulator.name = Field::ModulatorName.initial_text().into();
        match &mut modulator.source {
            ModulatorSource::BandEnergy { range, .. }
            | ModulatorSource::BandOnset { range, .. } => {
                *range = BandRange::Semitone {
                    low_midi: 24,
                    high_midi: 60,
                };
            }
        }
        project.modulators.push(modulator);
        let mut state = EditorState::default();
        state.reset_for_project(&project);
        state.select_layer_id(&project, layer_id);
        state.set_inspector_tab(field.tab());
        let context = egui::Context::default();
        // Match host setup: registration must precede the very first pass.
        install_input_policy(&context);
        let mut h = Self {
            context,
            project,
            state,
            layer: layer_id,
            field,
            time: 0.0,
            output: egui::FullOutput::default(),
            inspector: Rect::NOTHING,
            response: EditorResponse::default(),
            events_after_shortcut: Vec::new(),
            recovery_before_shortcut: None,
            wants_keyboard_before_shortcut: false,
        };
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.time += 0.1;
        self.output = self.context.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 1600.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ctx| {
                self.wants_keyboard_before_shortcut = ctx.wants_keyboard_input();
                self.recovery_before_shortcut = egui_ime::recovering_text_input(ctx);
                let mut prepared = self.state.begin_frame(ctx, &mut self.project);
                // Observe before widgets can consume native Undo events.
                self.events_after_shortcut = ctx.input(|input| input.events.clone());
                egui::TopBottomPanel::top("ime-history")
                    .show(ctx, |ui| prepared.history_controls(ui));
                egui::TopBottomPanel::bottom("ime-timeline")
                    .default_height(250.0)
                    .show(ctx, |ui| {
                        prepared.timeline(ui, 0.0, None);
                    });
                let mut frame = prepared.properties();
                egui::SidePanel::left("ime-layers")
                    .default_width(220.0)
                    .show(ctx, |ui| frame.layer_list(ui));
                self.inspector = egui::SidePanel::right("ime-inspector")
                    .default_width(450.0)
                    .show(ctx, |ui| frame.inspector(ui, 0.0, &[], &[]))
                    .response
                    .rect;
                self.response = frame.finish();
                egui::CentralPanel::default().show(ctx, |_| {});
            },
        );
    }

    fn text_rect(&self, text: &str) -> Rect {
        fn find(shape: &egui::Shape, text: &str, inspector: Rect) -> Option<Rect> {
            match shape {
                egui::Shape::Vec(children) => children
                    .iter()
                    .rev()
                    .find_map(|shape| find(shape, text, inspector)),
                egui::Shape::Text(shape) if shape.galley.text() == text => {
                    let rect = shape.galley.rect.translate(shape.pos.to_vec2());
                    inspector.contains(rect.center()).then_some(rect)
                }
                _ => None,
            }
        }
        self.output
            .shapes
            .iter()
            .rev()
            .find_map(|shape| find(&shape.shape, text, self.inspector))
            .unwrap_or_else(|| panic!("missing inspector text {text:?} for {:?}", self.field))
    }

    fn pointer(&mut self, pos: Pos2, pressed: bool) {
        self.frame(vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            },
        ]);
    }

    fn focus(&mut self) -> egui::Id {
        let id = if matches!(self.field, Field::Lyrics) {
            self.state.lyrics_editors[&self.layer].widget_id(self.layer)
        } else {
            let pos = self.text_rect(self.field.initial_text()).center();
            self.pointer(pos, true);
            self.pointer(pos, false);
            self.context
                .memory(|memory| memory.focused())
                .expect("clicked real field")
        };
        let mut native = egui::text_edit::TextEditState::load(&self.context, id)
            .expect("real inspector TextEdit state");
        native
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(self.field.initial_text().chars().count()),
            )));
        native.store(&self.context, id);
        self.context.memory_mut(|memory| memory.request_focus(id));
        self.frame(vec![]);
        self.assert_focus(id);
        if !matches!(self.field, Field::Lyrics) {
            assert_eq!(
                egui_ime::focused_text_input(&self.context),
                Some(id),
                "the actual {:?} field must publish the shared helper contract",
                self.field
            );
        }
        id
    }

    fn assert_focus(&self, id: egui::Id) {
        assert_eq!(
            self.context.memory(|memory| memory.focused()),
            Some(id),
            "{:?}",
            self.field
        );
    }

    fn body(&self) -> String {
        let layer = self
            .project
            .layers
            .iter()
            .find(|layer| layer.id == self.layer)
            .unwrap();
        match self.field {
            Field::LayerName => layer.name.clone(),
            Field::Text => {
                let LayerKind::Text(text) = &layer.kind else {
                    unreachable!()
                };
                text.text.clone()
            }
            Field::ModulatorName => self.project.modulators[0].name.clone(),
            Field::Lyrics => self.state.lyrics_editors[&self.layer].as_str().to_owned(),
            Field::LowNote | Field::HighNote => {
                panic!("note drafts are owned by ModulatorEditorState")
            }
        }
    }

    fn idle_for_native_undo(&mut self) {
        for _ in 0..12 {
            self.frame(vec![]);
        }
    }

    fn seed_project_history(&mut self) {
        for gain in [3.0, 6.0] {
            let checkpoint = self.state.begin_external_edit(&self.project);
            self.project.display_gain_db = gain;
            self.state.finish_external_edit(&self.project, checkpoint);
        }
        assert!(self.state.undo(&mut self.project));
        assert_eq!(self.state.history_depths(), (1, 1));
    }
}

fn key(key: Key, modifiers: Modifiers, pressed: bool, repeat: bool) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat,
        modifiers,
    }
}

fn press(key_code: Key) -> Event {
    key(key_code, Modifiers::NONE, true, false)
}

fn release(key_code: Key) -> Event {
    key(key_code, Modifiers::NONE, false, false)
}

fn ime(event: egui::ImeEvent) -> Event {
    Event::Ime(event)
}

fn start_preedit() -> Vec<Event> {
    vec![
        ime(egui::ImeEvent::Enabled),
        ime(egui::ImeEvent::Preedit("未確定".into())),
    ]
}

#[test]
fn editable_names_keep_focus_and_native_state_across_each_input_pass() {
    for field in [Field::LayerName, Field::ModulatorName] {
        let mut h = Harness::new(field);
        let id = h.focus();
        for text in ["a", "b"] {
            h.frame(vec![Event::Text(text.into())]);
            h.frame(vec![]);
            h.assert_focus(id);
            assert_eq!(egui_ime::focused_text_input(&h.context), Some(id));
            assert!(egui::text_edit::TextEditState::load(&h.context, id).is_some());
        }
        let prefix = h.body();
        h.frame(vec![ime(egui::ImeEvent::Enabled)]);
        for preedit in ["に", "にほ", "にほん"] {
            h.frame(vec![ime(egui::ImeEvent::Preedit(preedit.into()))]);
            h.frame(vec![]);
            h.assert_focus(id);
            assert_eq!(h.body(), format!("{prefix}{preedit}"));
        }
        h.frame(vec![ime(egui::ImeEvent::Commit("日本".into()))]);
        h.frame(vec![ime(egui::ImeEvent::Disabled)]);
        h.frame(vec![]);
        h.assert_focus(id);
        assert_eq!(h.body(), format!("{prefix}日本"));
    }
}

#[test]
fn actual_modulator_name_cancel_keeps_field_identity_across_preedit_passes() {
    let mut h = Harness::new(Field::ModulatorName);
    let id = h.focus();
    let before = h.project.clone();
    h.frame(start_preedit());
    assert_eq!(h.body(), "ime-modulator未確定");
    h.assert_focus(id);
    h.frame(vec![release(Key::Escape)]);
    h.assert_focus(id);
    h.frame(vec![ime(egui::ImeEvent::Disabled)]);
    h.assert_focus(id);
    assert_eq!(h.project, before);
}

#[test]
fn actual_five_singlelines_and_lyrics_cancel_preedit_without_changing_committed_data() {
    for field in SINGLELINES.into_iter().chain([Field::Lyrics]) {
        for windows_cancel_burst in [false, true] {
            let mut h = Harness::new(field);
            let id = h.focus();
            let before = h.project.clone();
            let identity = h
                .state
                .lyrics_editors
                .get(&h.layer)
                .map(|editor| editor.identity_snapshot());
            h.frame(start_preedit());
            h.assert_focus(id);
            if !matches!(field, Field::LowNote | Field::HighNote) {
                assert!(h.body().ends_with("未確定"), "preedit reached {field:?}");
            }
            h.frame(vec![release(Key::Escape)]);
            h.assert_focus(id);
            assert!(!h.events_after_shortcut.contains(&press(Key::Escape)));
            assert!(h.events_after_shortcut.contains(&key(
                Key::Escape,
                Modifiers::NONE,
                false,
                false
            )));
            let cancel = if windows_cancel_burst {
                vec![
                    ime(egui::ImeEvent::Disabled),
                    ime(egui::ImeEvent::Disabled),
                    ime(egui::ImeEvent::Commit(String::new())),
                    ime(egui::ImeEvent::Disabled),
                    ime(egui::ImeEvent::Disabled),
                ]
            } else {
                vec![ime(egui::ImeEvent::Disabled)]
            };
            h.frame(cancel);
            h.assert_focus(id);
            assert_eq!(h.project, before, "cancel restores committed {field:?}");
            h.text_rect(field.initial_text());
            assert_eq!(
                h.state
                    .lyrics_editors
                    .get(&h.layer)
                    .map(|editor| editor.identity_snapshot()),
                identity
            );
            if matches!(field, Field::Lyrics) {
                assert_eq!(h.state.lyrics_editors[&h.layer].widget_id(h.layer), id);
                assert!(!h.state.has_pending_lyrics_edit());
            }
        }
    }
}

#[test]
fn actual_fields_tab_shift_tab_and_repeat_never_traverse_with_or_without_ime() {
    // Ordinary bridge-delivered Tab presses exercise the global product policy,
    // even while composing. IME-consumed candidate Tab is tested separately.
    for field in SINGLELINES.into_iter().chain([Field::Lyrics]) {
        for composing in [false, true] {
            let mut h = Harness::new(field);
            let id = h.focus();
            if composing {
                h.frame(start_preedit());
            }
            let before = h.project.clone();
            for modifiers in [Modifiers::NONE, Modifiers::SHIFT] {
                for repeat in [false, true] {
                    let event = key(Key::Tab, modifiers, true, repeat);
                    h.frame(vec![event.clone()]);
                    h.assert_focus(id);
                    assert!(
                        h.events_after_shortcut.iter().any(|observed| matches!(
                            observed, Event::Key { key: Key::Tab, pressed: true,
                                modifiers: observed_modifiers, repeat: observed_repeat, .. }
                                if *observed_modifiers == modifiers && *observed_repeat == repeat
                        )),
                        "shared focus policy leaves Tab presses intact for {field:?}, composing={composing}, repeat={repeat}"
                    );
                    assert_eq!(h.project, before, "Tab must not edit {field:?}");
                }
                h.frame(vec![key(Key::Tab, modifiers, false, false)]);
            }
        }
    }
}

#[test]
fn tab_in_start_and_commit_passes_keeps_the_real_text_field_owner() {
    for field in [Field::Text, Field::Lyrics] {
        let mut h = Harness::new(field);
        let id = h.focus();
        let mut events = start_preedit();
        // Candidate selection is consumed by the IME before egui; only the
        // release and the updated preedit reach these widgets.
        events.push(release(Key::Tab));
        h.frame(events);
        h.assert_focus(id);
        h.frame(vec![
            ime(egui::ImeEvent::Commit("確定".into())),
            release(Key::Tab),
        ]);
        h.assert_focus(id);
        assert_eq!(h.body(), format!("{}確定", field.initial_text()));
    }
}

#[test]
fn singleline_enter_commit_is_once_and_keeps_focus_with_same_or_later_release() {
    for same_pass in [false, true] {
        let mut h = Harness::new(Field::Text);
        let id = h.focus();
        h.frame(start_preedit());
        let mut events = vec![
            ime(egui::ImeEvent::Disabled),
            ime(egui::ImeEvent::Commit("確定".into())),
            ime(egui::ImeEvent::Disabled),
        ];
        if same_pass {
            events.push(release(Key::Enter));
        }
        h.frame(events);
        if !same_pass {
            h.frame(vec![release(Key::Enter)]);
        }
        assert_eq!(h.body(), "ime-body確定");
        h.assert_focus(id);
        h.frame(vec![Event::Text("続き".into())]);
        assert_eq!(h.body(), "ime-body確定続き");
    }
}

#[test]
fn raw_lyrics_enter_keeps_native_newline_semantics_and_commits_only_once() {
    for same_pass in [false, true] {
        let mut h = Harness::new(Field::Lyrics);
        let id = h.focus();
        h.frame(start_preedit());
        let mut events = vec![
            ime(egui::ImeEvent::Disabled),
            ime(egui::ImeEvent::Commit("確定".into())),
            ime(egui::ImeEvent::Disabled),
        ];
        if same_pass {
            events.push(release(Key::Enter));
        }
        h.frame(events);
        if !same_pass {
            h.frame(vec![release(Key::Enter)]);
        }
        h.assert_focus(id);
        // Process presses never reach the raw multiline widget. The commit
        // confirms text once without adding a newline.
        assert_eq!(h.body(), "original\n\nsecond確定");
        assert_eq!(h.body().matches("確定").count(), 1);
        assert!(!h.state.has_pending_lyrics_edit());
        let LayerKind::Lyrics(lyrics) = &h.project.layers.last().unwrap().kind else {
            unreachable!()
        };
        assert_eq!(lyrics.lines[0].time_secs, Some(3.0));
        assert_eq!(lyrics.lines[2].time_secs, Some(6.0));
    }
}

#[test]
fn ordinary_enter_finishes_singleline_and_lyrics_enter_after_commit_still_inserts_newline() {
    let mut h = Harness::new(Field::Text);
    let id = h.focus();
    h.frame(vec![press(Key::Enter)]);
    assert_ne!(h.context.memory(|memory| memory.focused()), Some(id));
    assert_eq!(h.body(), Field::Text.initial_text());

    let mut h = Harness::new(Field::Lyrics);
    let id = h.focus();
    h.frame(start_preedit());
    h.frame(vec![ime(egui::ImeEvent::Commit("確定".into()))]);
    h.frame(vec![press(Key::Enter)]);
    assert_eq!(h.body(), "original\n\nsecond確定\n");
    h.assert_focus(id);
}

#[test]
fn history_shortcuts_preserve_events_and_project_during_composition_and_grace_without_focus() {
    for composing in [false, true] {
        for (key_code, modifiers) in [
            (Key::Z, Modifiers::CTRL),
            (Key::Y, Modifiers::CTRL),
            (Key::Z, Modifiers::CTRL | Modifiers::SHIFT),
        ] {
            let mut h = Harness::new(Field::Text);
            h.seed_project_history();
            h.frame(start_preedit());
            if !composing {
                h.frame(vec![ime(egui::ImeEvent::Disabled)]);
            }
            assert!(egui_ime::ime_input_active(&h.context));
            assert_eq!(
                egui_ime::ime_composing_with_pending_events(
                    &h.context,
                    h.context.viewport_id(),
                    &[]
                ),
                composing
            );
            let before = h.project.clone();
            let resources = h.state.resources.clone();
            let depths = h.state.history_depths();
            let event = key(key_code, modifiers, true, false);
            h.frame(vec![event.clone()]);
            assert!(!h.wants_keyboard_before_shortcut);
            assert_eq!(h.recovery_before_shortcut, None);
            assert!(h.events_after_shortcut.contains(&event));
            assert_eq!(h.project, before);
            assert_eq!(h.state.resources, resources);
            assert_eq!(h.state.history_depths(), depths);
            assert!(!h.response.history_changed);
        }
    }
}

#[test]
fn helper_recovery_owns_project_shortcuts_for_one_pass_without_ime_activity() {
    for (key_code, modifiers) in [
        (Key::Z, Modifiers::CTRL),
        (Key::Y, Modifiers::CTRL),
        (Key::Z, Modifiers::CTRL | Modifiers::SHIFT),
    ] {
        let mut h = Harness::new(Field::Text);
        h.seed_project_history();
        let id = h.focus();
        assert!(!egui_ime::ime_input_active(&h.context));
        h.context.memory_mut(|memory| memory.surrender_focus(id));
        let before = h.project.clone();
        let depths = h.state.history_depths();
        let event = key(key_code, modifiers, true, false);
        h.frame(vec![event.clone()]);
        assert!(!h.wants_keyboard_before_shortcut);
        assert_eq!(h.recovery_before_shortcut, Some(id));
        assert!(h.events_after_shortcut.contains(&event));
        assert_eq!(h.project, before);
        assert_eq!(h.state.history_depths(), depths);
        assert!(!h.response.history_changed);
        // A hidden field's stale recovery claim cannot suppress project history.
        h.frame(vec![key(key_code, modifiers, false, false)]);
        h.state.set_inspector_tab(InspectorTab::Project);
        h.frame(vec![]);
        h.frame(vec![event]);
        assert_eq!(h.recovery_before_shortcut, None);
        assert!(h.response.history_changed);
        assert_ne!(h.project, before);
    }
}

#[test]
fn lyrics_cancel_keeps_widget_identity_cues_and_native_undo_ownership() {
    let mut h = Harness::new(Field::Lyrics);
    h.seed_project_history();
    let id = h.focus();
    let original_identity = h.state.lyrics_editors[&h.layer].identity_snapshot();
    h.idle_for_native_undo();
    h.frame(vec![Event::Text(" edited".into())]);
    h.idle_for_native_undo();
    let committed = h.project.clone();
    h.frame(start_preedit());
    h.frame(vec![release(Key::Escape)]);
    h.frame(vec![ime(egui::ImeEvent::Disabled)]);
    assert_eq!(h.project, committed);
    assert_eq!(h.state.lyrics_editors[&h.layer].widget_id(h.layer), id);
    h.idle_for_native_undo();
    let depths = h.state.history_depths();
    let undo = key(Key::Z, Modifiers::CTRL | Modifiers::COMMAND, true, false);
    h.frame(vec![undo.clone()]);
    assert!(h.events_after_shortcut.contains(&undo));
    assert_eq!(h.body(), Field::Lyrics.initial_text());
    assert_eq!(
        h.state.lyrics_editors[&h.layer].identity_snapshot(),
        original_identity
    );
    assert_eq!(h.project.display_gain_db, 3.0);
    assert_eq!(h.state.history_depths(), depths);
    assert!(!h.response.history_changed);
    assert!(!h.state.has_pending_lyrics_edit());
    h.assert_focus(id);
}
