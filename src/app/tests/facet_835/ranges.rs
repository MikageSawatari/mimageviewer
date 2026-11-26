use super::*;

fn media_fixture() -> (Harness, PathBuf) {
    let mut env = setup_app();
    configure(&mut env);
    let folder = env.tmp.path().join("ranges");
    std::fs::create_dir(&folder).unwrap();
    // Sparse physical files give four size buckets without allocating 100 MB.
    // Duration cache seeding follows duration_762::cache_media. The completion
    // proof itself is produced by the real metadata planner, never assigned.
    for (name, len) in [
        ("a.wav", 512),
        ("b.wav", 2 * 1024 * 1024),
        ("c.wav", 20 * 1024 * 1024),
        ("d.wav", 101 * 1024 * 1024),
    ] {
        std::fs::File::create(folder.join(name))
            .unwrap()
            .set_len(len)
            .unwrap();
    }
    env.load_folder_and_finish_for_test(folder.clone());
    let mut h = Harness::new(env);
    h.adopt(&folder, 4);
    for (name, seconds) in [
        ("a.wav", 30.0),
        ("b.wav", 300.0),
        ("c.wav", 1800.0),
        ("d.wav", 7200.0),
    ] {
        let idx = h.idx(name);
        let key = h.env.details_lazy_cache_key(idx).unwrap();
        let (source_mtime, source_size) = h.env.image_metas[idx].unwrap();
        h.env.details_lazy_meta.insert(
            key,
            DetailsLazyMeta {
                source_mtime,
                source_size,
                media: DetailsMediaMeta::Read(DetailsVideoProbe {
                    duration_secs: Some(seconds),
                    dims: None,
                    codec: None,
                }),
                ..Default::default()
            },
        );
    }
    h.env.request_duration_facet_load();
    h.until(|h| h.env.duration_scan_ready() && h.idle(), |_| {});
    h.env.selected = Some(h.idx("d.wav"));
    (h, folder)
}

#[derive(Clone, Copy)]
enum Range {
    Duration,
    Size,
}

impl Range {
    fn label(self) -> &'static str {
        match self {
            Self::Duration => "長さ",
            Self::Size => "サイズ",
        }
    }
    fn value(self, app: &App) -> Option<u32> {
        match self {
            Self::Duration => match app.settings.facet_filter.duration_preset {
                Some(FacetDurationPreset::Range { min, .. }) => min,
                _ => panic!("duration range not selected"),
            },
            Self::Size => match app.settings.facet_filter.size_preset {
                Some(FacetSizePreset::Range { min, .. }) => min.map(|value| value.value),
                _ => panic!("size range not selected"),
            },
        }
    }
}

struct Contract {
    generation: u64,
    selected: Option<usize>,
    range: Range,
    field: Option<egui::Id>,
    expected: Option<u32>,
    editing: bool,
}

impl Contract {
    fn check(&self, h: &Harness) {
        let diagnostic = h.diagnostic();
        // §7 T1's six invariants are checked after EVERY input and idle frame.
        assert!(
            egui::Popup::is_any_open(&h.ctx),
            "1 menu closed: {diagnostic}"
        );
        assert!(
            h.nodes.iter().any(|(_, node)| node.label() == Some("下限")),
            "2 range row not drawn: {diagnostic}"
        );
        assert_eq!(
            self.range.value(&h.env),
            self.expected,
            "2 field value: {diagnostic}"
        );
        // Selection/checking precede entry into the DragValue's editing mode;
        // neither operation promises TextEdit focus. Once clicked, require it
        // through every character and all asynchronous completion frames.
        if let Some(id) = self.field {
            let response = h.response(id);
            assert_eq!(
                response.enabled,
                self.expected.is_some(),
                "2 field enabled: {diagnostic}"
            );
            if self.editing {
                assert!(
                    response.focused && h.ctx.memory(|memory| memory.has_focus(id)),
                    "2 lost field focus: {diagnostic}"
                );
                let cursor = egui::TextEdit::load_state(&h.ctx, id)
                    .and_then(|state| state.cursor.char_range())
                    .expect("DragValue TextEdit cursor");
                let text = self.expected.unwrap().to_string();
                assert_eq!(
                    cursor.as_sorted_char_range().end.0,
                    text.len(),
                    "2 caret/draft: {diagnostic}"
                );
                let node = &h
                    .nodes
                    .iter()
                    .find(|(node, _)| widget_id(*node) == id)
                    .unwrap()
                    .1;
                // DragValue's numeric accessibility property is sampled before
                // TextEdit applies this character. Its text value is the actual
                // draft from this draw, including on the very first input frame.
                assert_eq!(
                    node.value(),
                    Some(text.as_str()),
                    "2 drawn draft: {diagnostic}"
                );
            }
        }
        assert_eq!(
            h.env.items_generation, self.generation,
            "3 items reloaded: {diagnostic}"
        );
        assert!(
            !h.env.direct_navigation_scan_pending() && h.env.folder_refresh_pending.is_none(),
            "4 rescan admitted a reload: {diagnostic}"
        );
        assert!(
            !h.env.sidecar_restore_active()
                && h.env.modal_dialog_block_reason() != Some("sidecar_restore"),
            "5 sidecar modal: {diagnostic}"
        );
        assert!(
            !probe::gates()
                .iter()
                .any(|gate| gate.reason == Some("sidecar_restore")),
            "5 transient sidecar modal: {diagnostic}"
        );
        assert_eq!(
            h.env.selected, self.selected,
            "6 selection moved: {diagnostic}"
        );
        assert!(
            !h.env.projected_viewer_session_open(),
            "6 grid opened an item: {diagnostic}"
        );
        assert!(
            probe::take_actions().is_empty(),
            "6 keyboard action escaped to grid: {diagnostic}"
        );
    }

    fn settle(&self, h: &mut Harness) {
        self.check(h);
        h.until(Harness::idle, |h| self.check(h));
    }
}

fn lower_drag(h: &Harness) -> egui::Id {
    let checkbox = h.labelled("下限").accesskit_id();
    let ids: Vec<_> = h
        .nodes
        .iter()
        .filter(|(_, node)| {
            node.role() == egui::Role::SpinButton && node.labelled_by().contains(&checkbox)
        })
        .map(|(id, _)| widget_id(*id))
        .collect();
    assert_eq!(
        ids.len(),
        1,
        "lower DragValue's actual response: {}",
        h.diagnostic()
    );
    ids[0]
}

fn type_range(range: Range) {
    let (mut h, _) = media_fixture();
    h.stage = format!("{}: open", range.label());
    h.open_menu(range.label());
    h.settle();
    let mut contract = Contract {
        generation: h.env.items_generation,
        selected: h.env.selected,
        range,
        field: None,
        expected: None,
        editing: false,
    };

    h.stage = format!("{}: select 範囲指定", range.label());
    h.click(h.labelled("範囲指定"));
    // The selection draw creates these rows. Wait one response-observation draw
    // in click(), then validate, including the disabled (unchecked) DragValue.
    contract.field = Some(lower_drag(&h));
    contract.settle(&mut h);

    h.stage = format!("{}: check 下限", range.label());
    h.click(h.labelled("下限"));
    contract.expected = Some(match range {
        Range::Duration => 1,
        Range::Size => 100,
    });
    contract.settle(&mut h);

    h.stage = format!("{}: click DragValue", range.label());
    h.click(contract.field.unwrap());
    // A real click selects all existing text; do not assign focus/cursor/draft.
    contract.editing = true;
    contract.settle(&mut h);
    for (character, value) in [("2", 2), ("5", 25)] {
        h.stage = format!("{}: Event::Text({character:?}) -> {value}", range.label());
        contract.expected = Some(value);
        h.tick(vec![egui::Event::Text(character.into())]);
        contract.settle(&mut h);
    }
    assert_eq!(range.value(&h.env), Some(25));
    // A navigation key is observable even without Text's numeric parsing.
    h.stage = format!("{}: caret Left stays in editor", range.label());
    h.tick(vec![key(egui::Key::ArrowLeft, true, egui::Modifiers::NONE)]);
    assert!(
        matches!(
            h.env.keyboard_owner_for_pass(&h.ctx),
            crate::keyboard_input::KeyboardOwner::TextInput { widget_id, .. }
                if widget_id == contract.field.unwrap()
        ),
        "range editor owns the navigation key: {}",
        h.diagnostic()
    );
    assert_eq!(h.env.selected, contract.selected, "{}", h.diagnostic());
    assert!(
        h.ctx
            .memory(|memory| memory.has_focus(contract.field.unwrap()))
    );
    assert_eq!(
        egui::TextEdit::load_state(&h.ctx, contract.field.unwrap())
            .and_then(|state| state.cursor.char_range())
            .unwrap()
            .as_sorted_char_range()
            .end
            .0,
        1,
        "Left moves the editor caret: {}",
        h.diagnostic()
    );
    h.tick(vec![key(
        egui::Key::ArrowLeft,
        false,
        egui::Modifiers::NONE,
    )]);
    assert_eq!(h.env.selected, contract.selected);
    assert!(!h.env.projected_viewer_session_open());
    h.stage = format!(
        "{}: Enter commits the field, not a grid open",
        range.label()
    );
    h.tick(vec![key(egui::Key::Enter, true, egui::Modifiers::NONE)]);
    assert_eq!(h.env.selected, contract.selected, "{}", h.diagnostic());
    assert!(!h.env.projected_viewer_session_open(), "{}", h.diagnostic());
    h.tick(vec![key(egui::Key::Enter, false, egui::Modifiers::NONE)]);
    assert_eq!(range.value(&h.env), Some(25));
    assert_eq!(h.env.items_generation, contract.generation);
    assert!(!h.env.projected_viewer_session_open());
}

#[test]
fn facet_835_t1_duration_range_types_each_character_in_real_frames() {
    type_range(Range::Duration);
}

#[test]
fn facet_835_t2_size_range_types_each_character_in_real_frames() {
    type_range(Range::Size);
}

struct DateEditorContract {
    generation: u64,
    selected: Option<usize>,
    field: egui::Id,
    year: bool,
}

impl DateEditorContract {
    fn check(&self, h: &Harness, draft: &str, caret: usize) {
        use crate::settings::FacetDatePreset;

        let diagnostic = h.diagnostic();
        assert!(
            egui::Popup::is_any_open(&h.ctx),
            "menu closed: {diagnostic}"
        );
        assert!(
            h.response(self.field).focused,
            "editor lost focus: {diagnostic}"
        );
        assert_eq!(
            h.env.selected, self.selected,
            "grid selection: {diagnostic}"
        );
        assert_eq!(
            h.env.items_generation, self.generation,
            "listing reloaded: {diagnostic}"
        );
        assert!(
            !h.env.projected_viewer_session_open(),
            "item opened: {diagnostic}"
        );
        assert!(
            !h.env.direct_navigation_scan_pending() && h.env.folder_refresh_pending.is_none(),
            "rescan admitted a reload: {diagnostic}"
        );
        assert!(
            !h.env.sidecar_restore_active()
                && h.env.modal_dialog_block_reason() != Some("sidecar_restore"),
            "sidecar modal: {diagnostic}"
        );
        let node = &h
            .nodes
            .iter()
            .find(|(id, _)| widget_id(*id) == self.field)
            .unwrap()
            .1;
        assert_eq!(node.value(), Some(draft), "drawn draft: {diagnostic}");
        let cursor = egui::TextEdit::load_state(&h.ctx, self.field)
            .and_then(|state| state.cursor.char_range())
            .expect("date DragValue's actual TextEdit cursor");
        let range = cursor.as_sorted_char_range();
        assert_eq!(
            (range.start.0, range.end.0),
            (caret, caret),
            "caret: {diagnostic}"
        );
        let value = draft.parse::<i32>().unwrap();
        if self.year {
            // The model clamps incomplete years, while the actual editor must
            // retain every typed prefix. No focus, cursor or draft is assigned.
            assert!(
                matches!(h.env.settings.facet_filter.date_preset,
                    Some(FacetDatePreset::Range { start: Some(start), end: None })
                        if start.year == value.clamp(1970, 9999)),
                "year value: {diagnostic}"
            );
        } else {
            assert_eq!(
                h.env.settings.facet_filter.date_preset,
                Some(FacetDatePreset::CustomDays(value as u16)),
                "days value: {diagnostic}"
            );
        }
    }
}

fn type_date_numeric(year: bool) {
    let (mut h, _) = media_fixture();
    let generation = h.env.items_generation;
    let selected = h.env.selected;
    h.env.settings.toolbar_facet_filter_items = vec![ToolbarFacetFilterItem::Date];
    h.tick(Vec::new());
    h.tick(Vec::new()); // Observe the new menu button's real response.
    h.stage = "date: open menu".into();
    h.open_menu("日付");
    h.settle();
    let field = if year {
        h.stage = "date: select 期間を指定".into();
        h.click(h.labelled("期間を指定"));
        // Keep the upper bound absent so editing the lower year cannot swap
        // the bounds and replace the focused field's value.
        h.click(h.labelled("終了"));
        h.labelled("開始 年")
    } else {
        h.stage = "date: select 日数を指定".into();
        h.click(h.labelled("日数を指定"));
        let label = h.labelled("日数を指定").accesskit_id();
        let ids: Vec<_> = h
            .nodes
            .iter()
            .filter(|(_, node)| {
                node.role() == egui::Role::SpinButton && node.labelled_by().contains(&label)
            })
            .map(|(id, _)| widget_id(*id))
            .collect();
        assert_eq!(ids.len(), 1, "days DragValue response: {}", h.diagnostic());
        ids[0]
    };
    let contract = DateEditorContract {
        generation,
        selected,
        field,
        year,
    };
    // As in T1, locate the real DragValue response and click into its internal
    // TextEdit; probe::fields() does not identify these numeric editors.
    h.click(field);
    let mut draft = String::new();
    let text = if year { "1970" } else { "25" };
    for character in text.chars() {
        draft.push(character);
        h.stage = format!("date year={year}: Event::Text({character:?}), draft={draft}");
        h.tick(vec![egui::Event::Text(character.to_string())]);
        contract.check(&h, &draft, draft.len());
        h.until(Harness::idle, |h| contract.check(h, &draft, draft.len()));
    }
    h.stage = format!("date year={year}: caret Left stays in editor");
    h.tick(vec![key(egui::Key::ArrowLeft, true, egui::Modifiers::NONE)]);
    assert!(
        matches!(h.env.keyboard_owner_for_pass(&h.ctx),
            crate::keyboard_input::KeyboardOwner::TextInput { widget_id, .. } if widget_id == field),
        "date editor owns Left: {}",
        h.diagnostic()
    );
    contract.check(&h, &draft, draft.len() - 1);
    h.tick(vec![key(
        egui::Key::ArrowLeft,
        false,
        egui::Modifiers::NONE,
    )]);
    h.until(Harness::idle, |h| {
        contract.check(h, &draft, draft.len() - 1)
    });
}

#[test]
fn facet_835_date_range_year_types_each_character_and_owns_left() {
    type_date_numeric(true);
}

#[test]
fn facet_835_custom_days_types_each_character_and_owns_left() {
    type_date_numeric(false);
}

#[test]
fn facet_835_color_tolerance_number_editor_owns_left_in_real_frames() {
    // Real media rows keep a meaningful list selection while the color menu
    // exercises its numeric editor; these rows are not color-scan candidates.
    let (mut h, _) = media_fixture();
    let generation = h.env.items_generation;
    let selected = h.env.selected;
    assert!(selected.is_some());
    h.env.settings.toolbar_facet_filter_items = vec![ToolbarFacetFilterItem::Color];
    h.tick(Vec::new());
    h.tick(Vec::new());
    h.stage = "color: open 画像色".into();
    h.open_menu("画像色");
    h.settle();
    // The Slider rail and label have separate ids. Locate the actual internal
    // DragValue via its AccessKit role and label, then click that response.
    let fields: Vec<_> = h
        .nodes
        .iter()
        .filter(|(_, node)| {
            node.role() == egui::Role::SpinButton
                && node.labelled_by().iter().any(|label| {
                    h.nodes
                        .iter()
                        // egui exposes a Role::Label's text as its value.
                        .any(|(id, node)| id == label && node.value() == Some("許容範囲"))
                })
        })
        .map(|(id, _)| widget_id(*id))
        .collect();
    assert_eq!(
        fields.len(),
        1,
        "tolerance number field: {}",
        h.diagnostic()
    );
    let field = fields[0];
    h.click(field);
    let check = |h: &Harness, draft: &str, caret: usize| {
        let diagnostic = h.diagnostic();
        assert!(
            egui::Popup::is_any_open(&h.ctx),
            "menu closed: {diagnostic}"
        );
        assert!(
            h.response(field).focused,
            "number field lost focus: {diagnostic}"
        );
        assert!(
            h.env.color_filter.input_has_focus,
            "color owner: {diagnostic}"
        );
        assert_eq!(h.env.selected, selected, "grid selection: {diagnostic}");
        assert_eq!(
            h.env.items_generation, generation,
            "listing reloaded: {diagnostic}"
        );
        assert!(
            !h.env.projected_viewer_session_open(),
            "item opened: {diagnostic}"
        );
        let node = &h
            .nodes
            .iter()
            .find(|(id, _)| widget_id(*id) == field)
            .unwrap()
            .1;
        assert_eq!(node.value(), Some(draft), "drawn draft: {diagnostic}");
        let cursor = egui::TextEdit::load_state(&h.ctx, field)
            .and_then(|state| state.cursor.char_range())
            .unwrap()
            .as_sorted_char_range();
        assert_eq!(
            (cursor.start.0, cursor.end.0),
            (caret, caret),
            "caret: {diagnostic}"
        );
        assert_eq!(
            h.env.color_filter.tolerance,
            draft.parse::<f32>().unwrap().clamp(
                crate::color_search::MIN_TOLERANCE,
                crate::color_search::MAX_TOLERANCE
            ),
            "tolerance value: {diagnostic}"
        );
    };
    let mut draft = String::new();
    for character in ['2', '5'] {
        draft.push(character);
        h.stage = format!("color tolerance: Event::Text({character:?})");
        h.tick(vec![egui::Event::Text(character.to_string())]);
        check(&h, &draft, draft.len());
        h.until(Harness::idle, |h| check(h, &draft, draft.len()));
    }
    h.stage = "color tolerance: Left stays in number editor".into();
    h.tick(vec![key(egui::Key::ArrowLeft, true, egui::Modifiers::NONE)]);
    assert!(
        matches!(h.env.keyboard_owner_for_pass(&h.ctx),
        crate::keyboard_input::KeyboardOwner::TextInput { widget_id, .. } if widget_id == field),
        "tolerance number owns Left: {}",
        h.diagnostic()
    );
    check(&h, &draft, 1);
    h.tick(vec![key(
        egui::Key::ArrowLeft,
        false,
        egui::Modifiers::NONE,
    )]);
    h.until(Harness::idle, |h| check(h, &draft, 1));
}

#[test]
fn facet_835_t3_duration_presets_only_project_visible_rows() {
    let (mut h, _) = media_fixture();
    let generation = h.env.items_generation;
    for (&preset, expected) in FacetDurationPreset::all()
        .iter()
        .zip(["a.wav", "b.wav", "c.wav", "d.wav"])
    {
        h.stage = format!("duration preset {preset:?}");
        h.env.settings.facet_filter.duration_preset = Some(preset);
        h.env.rebuild_visible_indices();
        h.until(Harness::idle, |h| {
            assert_eq!(h.env.items_generation, generation, "{}", h.diagnostic());
            assert_eq!(h.visible_names(), [expected], "{}", h.diagnostic());
        });
    }
}

#[test]
fn facet_835_t3_size_presets_only_project_visible_rows() {
    let (mut h, _) = media_fixture();
    let generation = h.env.items_generation;
    for (&preset, expected) in FacetSizePreset::all()
        .iter()
        .zip(["a.wav", "b.wav", "c.wav", "d.wav"])
    {
        h.stage = format!("size preset {preset:?}");
        h.env.settings.facet_filter.size_preset = Some(preset);
        h.env.rebuild_visible_indices();
        h.until(Harness::idle, |h| {
            assert_eq!(h.env.items_generation, generation, "{}", h.diagnostic());
            assert_eq!(h.visible_names(), [expected], "{}", h.diagnostic());
        });
    }
}

#[test]
fn facet_835_t3_extension_changes_only_project_visible_rows() {
    let (mut h, _) = image_fixture();
    let generation = h.env.items_generation;
    for (extension, expected) in [("png", vec!["A.png", "B.png", "C.png"]), ("jpg", vec![])] {
        h.stage = format!("extension {extension}");
        h.env.settings.facet_filter.exts = [extension.to_string()].into_iter().collect();
        h.env.rebuild_visible_indices();
        h.until(Harness::idle, |h| {
            assert_eq!(h.env.items_generation, generation, "{}", h.diagnostic());
            assert_eq!(h.visible_names(), expected, "{}", h.diagnostic());
        });
    }
}

fn preset_without_menu(restored: bool) {
    // Isolate the existing length acquisition contract from the physical-watch
    // key regression (T1/T3/T8). Same in-memory media/stamp fixture as §1.762;
    // full App frames still own planning, completion and the final projection.
    let mut env = setup_media_duration_thumbnail_app();
    configure(&mut env);
    let preset = Some(FacetDurationPreset::Under1Minute);
    if restored {
        let parent = FacetRoute::root(FacetScope::path(&env.tmp.path().join("parent")));
        env.adopt_main_facet_route(parent.clone());
        env.settings.facet_filter.duration_preset = preset;
        let child_route = parent.child(FacetScope::path(&env.tmp.path().join("parent/child")));
        env.adopt_main_facet_route(child_route);
        assert!(env.facet_filter_suppressed());
        assert!(env.settings.facet_filter.duration_preset.is_none());
    }
    for (idx, seconds) in [(0, 30.0), (1, 600.0), (4, 90.0)] {
        let key = env.details_lazy_cache_key(idx).unwrap();
        let (source_mtime, source_size) = env.image_metas[idx].unwrap();
        env.details_lazy_meta.insert(
            key,
            DetailsLazyMeta {
                source_mtime,
                source_size,
                media: DetailsMediaMeta::Read(DetailsVideoProbe {
                    duration_secs: Some(seconds),
                    dims: None,
                    codec: None,
                }),
                ..Default::default()
            },
        );
    }
    let mut h = Harness::new(env);
    h.stage = if restored {
        "T7 restore saved duration preset"
    } else {
        "T7 apply duration preset without menu"
    }
    .into();
    if restored {
        assert!(h.env.restore_facet_filter_suppression());
    } else {
        h.env.settings.facet_filter.duration_preset = preset;
    }
    assert_eq!(h.env.settings.facet_filter.duration_preset, preset);
    assert!(
        !h.env.duration_facet_requested,
        "no menu request in this test"
    );
    assert!(!h.env.duration_scan_ready());
    h.env.rebuild_visible_indices();
    assert_eq!(
        h.env.visible_indices,
        [0, 1, 2, 3, 4],
        "unproved lengths leave rows visible"
    );
    h.until(
        |h| h.env.duration_scan_ready() && h.env.details_meta_pending.is_none(),
        |h| {
            assert!(
                !egui::Popup::is_any_open(&h.ctx),
                "T7 must not depend on menu drawing"
            );
        },
    );
    assert_eq!(
        h.env.visible_indices,
        [0],
        "T7 completed whole-listing proof applies the restored/direct preset"
    );
}

#[test]
fn facet_835_t7_restored_duration_preset_acquires_without_menu() {
    preset_without_menu(true);
}

#[test]
fn facet_835_t7_direct_duration_preset_acquires_without_menu() {
    preset_without_menu(false);
}
