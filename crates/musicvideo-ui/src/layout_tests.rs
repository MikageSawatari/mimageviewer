//! Headless egui input tests for the split layout and its transaction boundaries.
use super::*;
use egui::{Event, FullOutput, Modifiers, Pos2, Rect, Vec2};
use musicvideo_core::default_project;

struct Harness {
    context: egui::Context,
    project: Project,
    state: EditorState,
    time: f64,
    output: FullOutput,
    response: EditorResponse,
    panels: [Rect; 3],
    timeline_entry: Project,
    properties_entry: Project,
    project_path: Option<PathBuf>,
    screen_height: f32,
}

impl Harness {
    fn new() -> Self {
        let project = default_project(10.0);
        let mut state = EditorState::default();
        state.reset_for_project(&project);
        let mut harness = Self {
            context: crate::test_context(),
            timeline_entry: project.clone(),
            properties_entry: project.clone(),
            project,
            state,
            time: 0.0,
            output: FullOutput::default(),
            response: EditorResponse::default(),
            panels: [Rect::NOTHING; 3],
            project_path: None,
            screen_height: 1000.0,
        };
        harness.frame(vec![]);
        harness.frame(vec![]);
        harness
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.time += 0.1;
        self.output = crate::headless_paint_output(self.context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(1280.0, self.screen_height),
                )),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ctx| {
                let mut prepared = self.state.begin_frame(ctx, &mut self.project);
                // Capture what the FIRST input region sees, not just final state.
                self.timeline_entry = prepared.project.clone();
                egui::Panel::top("history").show(ctx, |ui| prepared.history_controls(ui));
                self.panels[0] = egui::Panel::bottom("timeline")
                    .default_size(300.0)
                    .show(ctx, |ui| prepared.timeline(ui, 0.0, None))
                    .response
                    .rect;
                let mut frame = prepared.properties();
                self.properties_entry = frame.checkpoint.0.project.clone();
                self.panels[1] = egui::Panel::left("layers")
                    .default_size(220.0)
                    .show(ctx, |ui| frame.layer_list(ui))
                    .response
                    .rect;
                self.panels[2] = egui::Panel::right("inspector")
                    .default_size(360.0)
                    .show(ctx, |ui| {
                        frame.inspector_with_project_path(
                            ui,
                            0.0,
                            &[],
                            &[],
                            self.project_path.as_deref(),
                        )
                    })
                    .response
                    .rect;
                self.response = frame.finish();
                egui::CentralPanel::default().show(ctx, |_| {});
            },
        ));
    }

    fn label(&self, text: &str) -> Rect {
        self.shapes()
            .into_iter()
            .rev()
            .find_map(|shape| match shape {
                egui::Shape::Text(shape) if shape.galley.text() == text => {
                    Some(shape.galley.rect.translate(shape.pos.to_vec2()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing rendered label: {text}"))
    }

    fn opacity_track(&self) -> Rect {
        let label = self.label("不透明度");
        self.shapes()
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Rect(shape)
                    if shape.rect.width() > 80.0
                        && shape.rect.height() < 12.0
                        && (shape.rect.center().y - label.center().y).abs() < 4.0 =>
                {
                    Some(shape.rect)
                }
                _ => None,
            })
            .expect("rendered opacity slider track")
    }

    fn shapes(&self) -> Vec<&egui::Shape> {
        fn collect<'a>(shape: &'a egui::Shape, shapes: &mut Vec<&'a egui::Shape>) {
            match shape {
                egui::Shape::Vec(children) => {
                    for child in children {
                        collect(child, shapes);
                    }
                }
                shape => shapes.push(shape),
            }
        }
        let mut shapes = Vec::new();
        for shape in &self.output.shapes {
            collect(&shape.shape, &mut shapes);
        }
        shapes
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

    fn click(&mut self, label: &str) {
        let pos = self.label(label).center();
        self.pointer(pos, true);
        self.pointer(pos, false);
    }

    fn drag_opacity(&mut self, leave_panel: bool) {
        let track = self.opacity_track();
        let start = Pos2::new(track.center().x, track.center().y);
        self.pointer(start, true);
        self.frame(vec![Event::PointerMoved(start - Vec2::new(20.0, 0.0))]);
        let end = if leave_panel {
            self.panels[1].center()
        } else {
            start - Vec2::new(35.0, 0.0)
        };
        if leave_panel {
            self.frame(vec![Event::PointerMoved(end)]);
            assert!(
                self.state.history.has_continuous_edit(),
                "pointer gesture stays open while crossing panels"
            );
        }
        // The release carries a new position too: the last slider change must
        // belong to the same transaction, even though the list is drawn first.
        self.pointer(end, false);
    }

    fn shortcut(&mut self, key: egui::Key, modifiers: Modifiers) {
        self.frame(vec![Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }]);
        self.frame(vec![Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers,
        }]);
    }

    fn focused_widget(&self, panel: Rect) -> egui::Id {
        let id = self
            .context
            .memory(|memory| memory.focused())
            .expect("widget has focus");
        let response = self
            .context
            .read_response(id)
            .expect("focused widget was drawn");
        assert!(panel.contains(response.rect.center()));
        id
    }

    fn focus_gain_text(&mut self) -> egui::Id {
        let rect = self
            .shapes()
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Text(shape) if shape.galley.text().starts_with("表示ゲイン") => {
                    Some(shape.galley.rect.translate(shape.pos.to_vec2()))
                }
                _ => None,
            })
            .expect("gain DragValue");
        self.pointer(rect.center(), true);
        self.pointer(rect.center(), false);
        self.frame(vec![]); // DragValue now draws its native TextEdit.
        self.focused_widget(self.panels[2])
    }

    fn replace_focused_text(&mut self, text: &str) {
        for _ in 0..12 {
            self.frame(vec![]);
        }
        self.shortcut(egui::Key::A, Modifiers::CTRL | Modifiers::COMMAND);
        self.frame(vec![Event::Text(text.into())]);
        // Give TextEdit's own undoer a stable checkpoint, as in normal typing.
        for _ in 0..12 {
            self.frame(vec![]);
        }
    }
}

#[test]
fn image_fit_choices_use_project_history_and_dirty_tracking() {
    for fit in [Fit::Contain, Fit::Stretch, Fit::Cover] {
        let mut h = Harness::new();
        let index = h
            .project
            .layers
            .iter()
            .position(|layer| matches!(layer.kind, LayerKind::Image { .. }))
            .unwrap();
        let LayerKind::Image { fit: initial, .. } = &mut h.project.layers[index].kind else {
            unreachable!()
        };
        *initial = if fit == Fit::Cover {
            Fit::Contain
        } else {
            Fit::Cover
        };
        h.state.reset_for_project(&h.project);
        h.state.select_layer(&h.project, index);
        let before = h.project.clone();
        let mut document = DocumentSession::default();
        document.replace_with_loaded("saved.mivmv.json".into(), &h.project);
        h.frame(vec![]);
        let LayerKind::Image { fit: initial, .. } = before.layers[index].kind else {
            unreachable!()
        };
        h.click(image_fit_label(initial));
        h.frame(vec![]);
        h.frame(vec![]);
        h.click(image_fit_label(fit));
        assert!(h.response.project_changed);
        h.frame(vec![]);
        let mut expected = before.clone();
        let LayerKind::Image { fit: result, .. } = &mut expected.layers[index].kind else {
            unreachable!()
        };
        *result = fit;
        assert_eq!(h.project, expected);
        assert!(document.is_dirty(&h.project));
        assert_eq!(h.state.history_depths(), (1, 0));
        assert!(h.state.undo(&mut h.project));
        assert_eq!(h.project, before);
        assert!(!document.is_dirty(&h.project));
        assert!(h.state.redo(&mut h.project));
        assert_eq!(h.project, expected);
        assert!(document.is_dirty(&h.project));
    }
}

#[test]
fn layer_list_controls_stay_fixed_when_layers_grow_and_list_scrolls() {
    let mut h = Harness::new();
    let labels = [
        "レイヤー",
        "一覧の上が奥、下が手前です",
        "一覧の上へ（奥）",
        "一覧の下へ（手前）",
        "選択中のレイヤーを削除…",
        "＋ レイヤーを追加",
    ];
    // The heading shares its text with an inspector tab; identify it in the left panel.
    let list_rect = |h: &Harness, label: &str| {
        let text_rect = h
            .shapes()
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Text(shape)
                    if shape.galley.text() == label && h.panels[1].contains(shape.pos) =>
                {
                    Some(shape.galley.rect.translate(shape.pos.to_vec2()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing fixed list control: {label}"));
        if labels[2..].contains(&label) {
            h.shapes()
                .into_iter()
                .filter_map(|shape| match shape {
                    egui::Shape::Rect(shape) if shape.rect.contains_rect(text_rect) => {
                        Some(shape.rect)
                    }
                    _ => None,
                })
                .min_by(|a, b| a.area().total_cmp(&b.area()))
                .expect("rendered button rectangle")
        } else {
            text_rect
        }
    };
    let before: Vec<_> = labels.iter().map(|label| list_rect(&h, label)).collect();
    let first_name = h.project.layers[0].name.clone();
    let first_before = list_rect(&h, &first_name);
    assert!(
        first_before.top() > before.last().unwrap().bottom(),
        "only the names belong below the fixed controls"
    );
    for index in 0..80 {
        let mut layer = new_solid_layer(&h.project);
        layer.name = format!("extra-{index}");
        h.project.layers.push(layer);
    }
    h.frame(vec![]);
    h.frame(vec![]);
    for (label, expected) in labels.iter().zip(&before) {
        assert_eq!(list_rect(&h, label), *expected);
    }
    let visible_names = |h: &Harness| {
        h.shapes()
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(shape)
                    if shape.galley.text().starts_with("extra-")
                        && h.panels[1].contains(shape.pos) =>
                {
                    Some(shape.galley.text().to_owned())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let names_before_scroll = visible_names(&h);
    h.frame(vec![
        Event::PointerMoved(first_before.center()),
        Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, -600.0),
            modifiers: Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        },
    ]);
    for _ in 0..8 {
        h.frame(vec![]);
    }
    for (label, expected) in labels.iter().zip(&before) {
        assert_eq!(
            list_rect(&h, label),
            *expected,
            "fixed controls must not scroll"
        );
    }
    assert_ne!(
        visible_names(&h),
        names_before_scroll,
        "the list itself must scroll"
    );
}

#[test]
fn radial_inner_circle_uses_percent_and_shape_switch_default() {
    let mut h = Harness::new();
    h.screen_height = 1600.0;
    let index = h
        .project
        .layers
        .iter()
        .position(|layer| matches!(layer.kind, LayerKind::Spectrum(_)))
        .unwrap();
    let LayerKind::Spectrum(spectrum) = &mut h.project.layers[index].kind else {
        unreachable!()
    };
    spectrum.shape = SpectrumShape::Bars {
        gap_ratio: 0.3,
        min_height_px: 3.0,
        thickness_px: 12.0,
    };
    h.state.reset_for_project(&h.project);
    h.state.select_layer(&h.project, index);
    h.frame(vec![]);
    h.click("円形");
    h.frame(vec![]);
    let LayerKind::Spectrum(spectrum) = &h.project.layers[index].kind else {
        unreachable!()
    };
    let SpectrumShape::Radial { inner_ratio, .. } = spectrum.shape else {
        panic!("radial mode")
    };
    assert_eq!(inner_ratio, 380.0 / 550.0);
    h.label("バーは枠の端まで伸びます");
    let value = h.label("内側の円 ");
    h.label(" %");
    h.pointer(value.center(), true);
    h.pointer(value.center(), false);
    h.frame(vec![]);
    h.replace_focused_text("40");
    h.shortcut(egui::Key::Enter, Modifiers::NONE);
    let LayerKind::Spectrum(spectrum) = &h.project.layers[index].kind else {
        unreachable!()
    };
    let SpectrumShape::Radial { inner_ratio, .. } = spectrum.shape else {
        unreachable!()
    };
    assert!(
        (inner_ratio - 0.4).abs() < 1e-6,
        "40 percent is stored as a ratio"
    );
}

#[test]
fn output_fps_choices_use_project_history_and_dirty_tracking() {
    for fps in [24.0, 30.0, 60.0] {
        let mut h = Harness::new();
        assert_eq!(h.project.output.fps, 60.0);
        h.project.output.fps = if fps == 60.0 { 24.0 } else { 60.0 };
        h.state.reset_for_project(&h.project);
        h.state.set_inspector_tab(InspectorTab::Project);
        let before = h.project.clone();
        let mut document = DocumentSession::default();
        document.replace_with_loaded("saved.mivmv.json".into(), &h.project);
        h.frame(vec![]);
        assert!(h.panels[2].contains_rect(h.label("解像度: 1920 × 1080")));
        h.click(&format!("{} fps", before.output.fps));
        h.frame(vec![]);
        h.frame(vec![]);
        h.click(&format!("{fps} fps"));
        assert!(h.response.project_changed);
        h.frame(vec![]);
        assert_eq!(h.project.output.fps, fps);
        let mut expected = before.clone();
        expected.output.fps = fps;
        assert_eq!(h.project, expected);
        assert!(document.is_dirty(&h.project));
        assert_eq!(h.state.history_depths(), (1, 0));
        assert!(h.state.undo(&mut h.project));
        assert_eq!(h.project, before);
        assert!(!document.is_dirty(&h.project));
        assert!(h.state.redo(&mut h.project));
        assert_eq!(h.project, expected);
        assert!(document.is_dirty(&h.project));
    }
}

#[test]
fn output_fps_custom_value_is_displayed_and_preserved_until_selected() {
    let mut h = Harness::new();
    h.project.output.fps = 29.97;
    h.state.reset_for_project(&h.project);
    h.state.set_inspector_tab(InspectorTab::Project);
    let before = h.project.clone();
    h.frame(vec![]);
    h.frame(vec![]);
    let label = "29.97 fps（このプロジェクトの値）";
    assert!(h.panels[2].contains_rect(h.label(label)));
    assert_eq!(h.project, before);
    assert!(!h.response.project_changed);
    assert_eq!(h.state.history_depths(), (0, 0));
    h.click(label);
    h.frame(vec![]);
    h.frame(vec![]);
    for fps in [24, 30, 60] {
        h.label(&format!("{fps} fps"));
    }
    h.click("30 fps");
    assert!(h.response.project_changed);
    h.frame(vec![]);
    assert_eq!(h.project.output.fps, 30.0);
    assert!(h.state.undo(&mut h.project));
    assert_eq!(h.project, before);
}

#[test]
fn project_tab_paths_are_read_only_and_stay_in_the_scrollable_inspector() {
    let mut h = Harness::new();
    h.state.set_inspector_tab(InspectorTab::Project);
    h.project_path = Some(PathBuf::from("saved.mivmv.json"));
    h.project.audio = Some(musicvideo_core::AudioRef {
        path: "sound.wav".into(),
    });
    h.project.images = (0..40)
        .map(|index| musicvideo_core::ImageRef {
            id: ImageId::new(format!("image-{index}")),
            path: PathBuf::from(format!("image-{index}.png")),
        })
        .collect();
    let project = h.project.clone();
    h.frame(vec![]);
    h.frame(vec![]);
    for text in [
        "プロジェクト: saved.mivmv.json",
        "音声: sound.wav",
        "画像 image-0: image-0.png",
    ] {
        assert!(
            h.output.shapes.iter().any(
                |s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == text)
                    && h.panels[2].contains(s.clip_rect.center())
            ),
            "missing inspector path: {text}"
        );
    }
    assert_eq!(h.project, project);
    assert!(!h.response.project_changed);
    assert!(!h.state.can_undo());
}

#[test]
fn split_layout_widget_ids_are_distinct_between_tabs_and_layer_list() {
    let mut h = Harness::new();
    let modulator = musicvideo_core::new_band_energy_modulator(&h.project);
    let modulator_name = modulator.name.clone();
    h.project.modulators.push(modulator);
    h.frame(vec![]);
    let mut list_ids = std::collections::HashSet::new();
    let names: Vec<_> = h
        .project
        .layers
        .iter()
        .map(|layer| layer.name.clone())
        .collect();
    for name in names {
        let rect = h
            .shapes()
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Text(shape)
                    if shape.galley.text() == name && h.panels[1].contains(shape.pos) =>
                {
                    Some(shape.galley.rect.translate(shape.pos.to_vec2()))
                }
                _ => None,
            })
            .unwrap();
        h.pointer(rect.center(), true);
        h.pointer(rect.center(), false);
        let id = h
            .context
            .interaction_snapshot(|interaction| interaction.clicked)
            .expect("layer list item clicked");
        assert!(h.panels[1].contains(h.context.read_response(id).unwrap().rect.center()));
        assert!(list_ids.insert(id));
    }
    let name = h.project.layers[h.state.selected_layer(&h.project)]
        .name
        .clone();
    h.click(&name);
    let name_id = h.focused_widget(h.panels[2]);
    h.click("プロジェクト");
    let gain_id = h.focus_gain_text();
    h.click("モジュレータ");
    h.click(&modulator_name);
    let modulator_id = h.focused_widget(h.panels[2]);
    assert_ne!(name_id, gain_id);
    assert_ne!(name_id, modulator_id);
    assert_ne!(gain_id, modulator_id);
    for id in [name_id, gain_id, modulator_id] {
        assert!(!list_ids.contains(&id));
    }
    h.click("レイヤー");
    h.click(&name);
    assert_eq!(h.focused_widget(h.panels[2]), name_id);
    h.click("プロジェクト");
    assert_eq!(h.focus_gain_text(), gain_id);
}

#[test]
fn split_layout_tab_round_trip_preserves_each_fields_native_undo() {
    let mut h = Harness::new();
    let index = h.state.selected_layer(&h.project);
    let original_name = h.project.layers[index].name.clone();
    h.click(&original_name);
    h.replace_focused_text("renamed layer");
    assert_eq!(h.project.layers[index].name, "renamed layer");
    h.click("プロジェクト");
    h.focus_gain_text();
    h.replace_focused_text("12");
    h.shortcut(egui::Key::Enter, Modifiers::NONE);
    assert_eq!(h.project.display_gain_db, 12.0);
    h.click("レイヤー");
    h.click("renamed layer");
    // Native Undo also records cursor selection. A click after returning to the
    // tab may first undo that selection; every target must still belong here.
    for _ in 0..3 {
        h.shortcut(egui::Key::Z, Modifiers::CTRL | Modifiers::COMMAND);
        if h.project.layers[index].name == original_name {
            break;
        }
        assert_eq!(h.project.layers[index].name, "renamed layer");
    }
    assert_eq!(h.project.layers[index].name, original_name);
    assert_eq!(h.project.display_gain_db, 12.0);
    h.click("プロジェクト");
    h.focus_gain_text();
    for _ in 0..3 {
        h.shortcut(egui::Key::Z, Modifiers::CTRL | Modifiers::COMMAND);
        // egui 0.37 keeps DragValue's edit text in a private typed state.
        // Observe the actual numeric edit through the production model instead.
        let value = h.project.display_gain_db;
        if value == 0.0 {
            break;
        }
        assert_eq!(value, 12.0);
    }
    h.shortcut(egui::Key::Enter, Modifiers::NONE);
    assert_eq!(h.project.display_gain_db, 0.0);
    assert_eq!(h.project.layers[index].name, original_name);
}

#[test]
fn split_layout_reorder_then_property_drag_keeps_two_undo_steps_and_selection() {
    let mut h = Harness::new();
    let before = h.project.clone();
    let selected = h.state.selected_layer;
    h.click("一覧の上へ（奥）");
    let reordered = h.project.clone();
    assert_ne!(before, reordered);
    assert_eq!(h.state.history_depths(), (1, 0));
    h.drag_opacity(false);
    let edited = h.project.clone();
    assert_ne!(reordered, edited);
    assert_eq!(h.state.history_depths(), (2, 0));
    h.shortcut(egui::Key::Z, Modifiers::CTRL);
    assert_eq!(h.project, reordered);
    assert_eq!(h.state.selected_layer, selected);
    h.shortcut(egui::Key::Z, Modifiers::CTRL);
    assert_eq!(h.project, before);
    assert_eq!(h.state.selected_layer, selected);
    h.shortcut(egui::Key::Y, Modifiers::CTRL);
    assert_eq!(h.project, reordered);
    h.shortcut(egui::Key::Y, Modifiers::CTRL);
    assert_eq!(h.project, edited);
    // Both visible regions point to the restored ID, even after reordering.
    let layer = &h.project.layers[h.state.selected_layer(&h.project)];
    assert_eq!(Some(layer.id), selected);
    for panel in [h.panels[1], h.panels[2]] {
        assert!(h.shapes().iter().any(|shape| matches!(shape,
            egui::Shape::Text(shape) if shape.galley.text() == layer.name
                && panel.contains(shape.pos))));
    }
    assert_eq!(h.state.inspector_tab(), InspectorTab::Layer);
    h.opacity_track();
}

#[test]
fn split_layout_pointer_drag_released_over_left_panel_is_one_undo_step() {
    let mut h = Harness::new();
    let before = h.project.clone();
    h.drag_opacity(true);
    let after = h.project.clone();
    assert_ne!(before, after);
    assert_eq!(h.state.history_depths(), (1, 0));
    h.shortcut(egui::Key::Z, Modifiers::CTRL);
    assert_eq!(h.project, before);
    h.shortcut(egui::Key::Y, Modifiers::CTRL);
    assert_eq!(h.project, after);
}

#[test]
fn split_layout_tab_switch_is_not_history_and_modulator_edit_is_separate() {
    let mut h = Harness::new();
    let before = h.project.clone();
    h.drag_opacity(false);
    let properties = h.project.clone();
    h.click("モジュレータ");
    assert_eq!(h.state.history_depths(), (1, 0));
    h.click("＋ キックに合わせる動きを追加");
    let modulated = h.project.clone();
    assert_ne!(properties, modulated);
    assert_eq!(h.state.history_depths(), (2, 0));
    h.shortcut(egui::Key::Z, Modifiers::CTRL);
    assert_eq!(h.project, properties);
    assert_eq!(h.state.inspector_tab(), InspectorTab::Modulators);
    h.shortcut(egui::Key::Z, Modifiers::CTRL);
    assert_eq!(h.project, before);
    assert_eq!(h.state.inspector_tab(), InspectorTab::Modulators);
}

#[test]
fn split_layout_selection_keeps_tab_but_added_layer_opens_properties() {
    let mut h = Harness::new();
    h.click("プロジェクト");
    let first_name = h.project.layers[0].name.clone();
    h.click(&first_name);
    assert_eq!(h.state.selected_layer, Some(h.project.layers[0].id));
    assert_eq!(h.state.inspector_tab(), InspectorTab::Project);
    assert_eq!(h.state.history_depths(), (0, 0));
    h.click("＋ レイヤーを追加");
    h.frame(vec![]);
    h.click("テキスト");
    assert_eq!(h.state.inspector_tab(), InspectorTab::Layer);
    assert_eq!(
        h.state.selected_layer,
        h.project.layers.last().map(|layer| layer.id)
    );
    assert_eq!(h.state.history_depths(), (1, 0));
}

#[test]
fn split_layout_frame_finish_registers_lifetimes_for_ui_added_layers() {
    for (preset, added_count) in [("単色", 1), ("mIV スペクトラム + 鍵盤", 2)] {
        let mut h = Harness::new();
        let before = h.project.clone();
        let original: Vec<_> = before
            .layers
            .iter()
            .map(|layer| (layer.id, h.state.layer_target_lifetime(layer.id).unwrap()))
            .collect();
        h.click("プロジェクト");
        h.click("＋ レイヤーを追加");
        h.frame(vec![]);
        h.click(preset);
        // No subsequent begin_frame or external-edit helper is allowed here:
        // the host can request its preview immediately after finish returns.
        assert_eq!(h.project.layers.len(), before.layers.len() + added_count);
        for layer in &h.project.layers {
            assert!(
                h.state.layer_target_lifetime(layer.id).is_some(),
                "unregistered layer: {:?}",
                layer.id
            );
        }
        for (id, lifetime) in original {
            assert_eq!(h.state.layer_target_lifetime(id), Some(lifetime));
        }
        assert_eq!(h.state.history_depths(), (1, 0));
        assert!(h.state.undo(&mut h.project));
        assert_eq!(h.project, before);
    }
}

#[test]
fn split_layout_shortcut_precedes_timeline_and_runs_once() {
    let mut h = Harness::new();
    let before = h.project.clone();
    h.click("一覧の上へ（奥）");
    let reordered = h.project.clone();
    h.drag_opacity(false);
    h.frame(vec![Event::Key {
        key: egui::Key::Z,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::CTRL,
    }]);
    assert_eq!(h.timeline_entry, reordered);
    assert_eq!(h.project, reordered);
    assert_ne!(h.project, before);
    assert_eq!(h.state.history_depths(), (1, 1));
    assert!(h.response.history_changed);
}

#[test]
fn split_layout_lyrics_native_undo_does_not_move_project_history() {
    let mut h = Harness::new();
    let mut layer = new_lyrics_layer(&h.project);
    let id = layer.id;
    let LayerKind::Lyrics(lyrics) = &mut layer.kind else {
        unreachable!()
    };
    lyrics.lines = vec![LyricLine {
        text: "original".into(),
        time_secs: Some(1.0),
    }];
    h.project.layers.push(layer);
    h.state.reset_for_project(&h.project);
    h.state.select_layer(&h.project, h.project.layers.len() - 1);
    let checkpoint = h.state.begin_external_edit(&h.project);
    h.project.display_gain_db = 3.0;
    h.state.finish_external_edit(&h.project, checkpoint);
    h.frame(vec![]);
    let widget = h.state.lyrics_editors[&id].widget_id(id);
    let mut native = egui::text_edit::TextEditState::load(&h.context, widget).unwrap();
    native
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(8),
        )));
    native.store(&h.context, widget);
    h.context.memory_mut(|memory| memory.request_focus(widget));
    for _ in 0..12 {
        h.frame(vec![]);
    }
    h.frame(vec![Event::Text(" edited".into())]);
    for _ in 0..12 {
        h.frame(vec![]);
    }
    let depth = h.state.history_depths();
    h.shortcut(egui::Key::Z, Modifiers::CTRL | Modifiers::COMMAND);
    let LayerKind::Lyrics(lyrics) = &h.project.layers.last().unwrap().kind else {
        unreachable!()
    };
    assert_eq!(lyrics.lines[0].text, "original");
    assert_eq!(lyrics.lines[0].time_secs, Some(1.0));
    assert_eq!(h.project.display_gain_db, 3.0);
    assert_eq!(h.state.history_depths(), depth);
}

#[test]
fn split_layout_timeline_mutation_is_outside_properties_checkpoint() {
    let mut h = Harness::new();
    let id = h.state.selected_layer.unwrap();
    h.state.apply_external_command(
        &mut h.project,
        &EditorCommand::AddKey {
            layer: id,
            property: AnimProperty::Opacity,
            t: 2.0,
        },
    );
    h.frame(vec![]);
    h.frame(vec![]);
    let before = h.project.clone();
    let marker = h
        .shapes()
        .into_iter()
        .find_map(|shape| match shape {
            egui::Shape::Path(path)
                if path.closed
                    && path.points.len() == 4
                    && path.points.iter().all(|pos| h.panels[0].contains(*pos)) =>
            {
                Some(Rect::from_points(&path.points).center())
            }
            _ => None,
        })
        .expect("timeline key diamond");
    h.pointer(marker, true);
    h.frame(vec![Event::PointerMoved(marker + Vec2::new(6.5, 0.0))]);
    let end = marker + Vec2::new(100.0, 0.0);
    h.frame(vec![Event::PointerMoved(end)]);
    assert_eq!(
        h.properties_entry, h.project,
        "properties starts after this frame's timeline mutation"
    );
    h.pointer(end, false);
    let timeline_edit = h.project.clone();
    assert_ne!(before, timeline_edit);
    h.drag_opacity(false);
    assert_ne!(h.project, timeline_edit);
    h.shortcut(egui::Key::Z, Modifiers::CTRL);
    assert_eq!(h.project, timeline_edit);
    h.shortcut(egui::Key::Z, Modifiers::CTRL);
    assert_eq!(h.project, before);
}
