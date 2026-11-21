use super::*;

enum Field {
    Address,
    TagApply { absent: bool },
    CollectionName,
    SmartRule,
    SmartEditorRule,
}

impl Adapter for Field {
    fn label(&self) -> String {
        match self {
            Self::Address => "address/history",
            Self::TagApply { .. } => "tag suggestions",
            Self::CollectionName => "new collection name",
            Self::SmartRule => "add smart-folder rule",
            Self::SmartEditorRule => "edit smart-folder rule",
        }
        .into()
    }
    fn open(&self, app: &mut App, _ctx: &egui::Context) {
        match self {
            Self::Address => {} // The normal folder bar is already drawn; click its field.
            Self::TagApply { .. } => {
                app.move_grid_cursor(Some(0), GridCursorIntent::ExplicitKeep);
                app.open_tag_apply_dialog();
            }
            Self::CollectionName => app.open_collection_manager(None),
            Self::SmartRule => {
                app.begin_add_current_smart_folder_rule(app.settings.smart_folders[0].id)
            }
            Self::SmartEditorRule => {
                app.open_smart_folder_manager(Some(app.settings.smart_folders[0].id))
            }
        }
    }
    fn locator(&self) -> Locator {
        match self {
            Self::Address => Locator::Id(egui::Id::unique("folder_address_input")),
            Self::TagApply { .. } => {
                Locator::Site("src/ui_dialogs/tag_apply.rs", "&mut self.tag_apply_input")
            }
            Self::CollectionName => Locator::Site(
                "src/ui_dialogs/collections.rs",
                "&mut self.collection_ui.manager_new_name",
            ),
            Self::SmartRule => Locator::Site(
                "src/ui_dialogs/smart_folder_editor.rs",
                "&mut draft.filter.name_contains",
            ),
            Self::SmartEditorRule => Locator::Site(
                "src/ui_dialogs/smart_folder_editor.rs",
                "&mut rule.filter.name_contains",
            ),
        }
    }
    fn draft<'a>(&self, app: &'a App) -> &'a str {
        match self {
            Self::Address => &app.address,
            Self::TagApply { .. } => &app.tag_apply_input,
            Self::CollectionName => app.typing_collection_new_name_for_test(),
            // The pre-open source-adoption diagnostic has no editor draft yet.
            // assert_field separately requires the actual field to exist.
            Self::SmartRule => app
                .smart_folder_rule_draft
                .as_ref()
                .map(|draft| draft.filter.name_contains.as_str())
                .unwrap_or(""),
            Self::SmartEditorRule => app
                .smart_folder_editor_draft
                .as_ref()
                .map(|draft| draft.rules[0].filter.name_contains.as_str())
                .unwrap_or(""),
        }
    }
    fn opens_focused(&self) -> bool {
        false
    }
    fn opening_frame(&self) -> u64 {
        match self {
            Self::Address => ADDRESS_OPENING_FRAME,
            Self::TagApply { .. } => TAG_SUGGESTIONS_OPENING_FRAME,
            Self::CollectionName | Self::SmartRule | Self::SmartEditorRule => DIALOG_OPENING_FRAME,
        }
    }
    fn job(&self) -> Option<&'static str> {
        matches!(self, Self::TagApply { .. }).then_some("tag-apply")
    }
    fn outcome(&self, query: &str) -> Outcome {
        match self {
            Self::TagApply { absent } => Outcome::Delivered(if *absent {
                0
            } else if query.starts_with('か') || query.starts_with('k') {
                1
            } else {
                2
            }),
            _ => unreachable!(),
        }
    }
    fn adopted(&self, app: &App, query: &str, outcome: Outcome) -> bool {
        match self {
            Self::TagApply { .. } => matches!(outcome, Outcome::Delivered(count)
                if app.tag_apply_suggestion_key.as_deref() == Some(crate::tags_db::normalize_tag_key(query).as_str())
                    && app.tag_apply_suggestions.len() == count),
            _ => true,
        }
    }
    fn check_idle(&self, app: &App) {
        // Address/history have no completion producer. Smart-folder edits only
        // update the draft/summary; no count-preview query is started. Collection
        // name checks trim/nonempty locally; duplicate validation waits for Create.
        if !matches!(self, Self::TagApply { .. }) {
            assert!(
                app.search_pending.is_none()
                    && app.favsearch_pending.is_none()
                    && app.tag_view_pending.is_none()
                    && app.global_search.pending.is_none(),
                "unexpected search while editing {}",
                self.label()
            );
        }
    }
}

fn harness(field: Field) -> Harness<Field> {
    harness_with_measurement(field, false)
}

fn harness_with_measurement(field: Field, measure: bool) -> Harness<Field> {
    let mut env = fixture();
    let paths = super::search::seed_images(&mut env);
    if matches!(field, Field::Address) {
        env.active_quick_folder_slot = None;
        env.recent_folders = ["alpha-history", "alpine-history"]
            .into_iter()
            .map(|name| {
                let path = env.tmp.path().join(name);
                std::fs::create_dir_all(&path).unwrap();
                path
            })
            .collect();
    }
    if matches!(field, Field::TagApply { .. }) {
        env.settings.tags = ["alpha", "alpine", "かな本", "koi"]
            .into_iter()
            .map(|name| crate::settings::TagDef::new(name.into()))
            .collect();
    }
    if matches!(field, Field::CollectionName) {
        let runtime = crate::collection_store::CollectionStoreRuntime::start_at(
            env.tmp.path().join("typing-collections.db"),
        )
        .unwrap();
        env.install_collection_runtime(runtime);
    }
    if matches!(field, Field::SmartRule | Field::SmartEditorRule) {
        let mut definition = crate::settings::SmartFolderDefinition::new("typing");
        definition.rules.push(crate::settings::SmartFolderRule::new(
            paths[0].parent().unwrap().to_path_buf(),
            false,
            Default::default(),
        ));
        env.settings.smart_folders = vec![definition];
    }
    Harness::new_with_measurement(env, field, paths, measure)
}

pub(super) fn measure_openings(failures: &mut Vec<String>) {
    for field in [
        Field::Address,
        Field::TagApply { absent: false },
        Field::CollectionName,
        Field::SmartRule,
        Field::SmartEditorRule,
    ] {
        let h = harness_with_measurement(field, true);
        if let Some(failure) = h.measured_opening_contract_failure() {
            failures.push(failure);
        }
    }
}

#[test]
fn typing_address_ascii() {
    harness(Field::Address).type_text("alp");
}
#[test]
fn typing_address_zero() {
    harness(Field::Address).type_text("rzz");
}
#[test]
fn typing_address_ime() {
    harness(Field::Address).compose_ime(&["k", "か", "かな"], "かな");
}
#[test]
fn typing_address_unicode_text() {
    harness(Field::Address).type_text("かな本");
}

#[test]
fn typing_history_popup_edit_keeps_field() {
    // ui_history_field is an explicitly opened, unfiltered history list, not
    // autocomplete. Editing closes it and keeps ownership in its real caller.
    let mut h = harness(Field::Address);
    assert_eq!(
        h.env
            .recent_folder_entries()
            .iter()
            .filter(|path| { h.env.current_folder.as_ref() != Some(*path) })
            .count(),
        2
    );
    h.stage = "open history then edit".into();
    h.tick(vec![key(egui::Key::ArrowDown, true, egui::Modifiers::NONE)]);
    h.tick(vec![key(
        egui::Key::ArrowDown,
        false,
        egui::Modifiers::NONE,
    )]);
    assert!(
        crate::ui_history_field::is_open_for(&h.ctx, h.id),
        "{}",
        h.diagnostic()
    );
    h.assert_field();
    h.type_text("alp");
    assert!(
        !crate::ui_history_field::is_open_for(&h.ctx, h.id),
        "editing closes history"
    );
}

#[test]
fn typing_tag_apply_suggestions_ascii() {
    harness(Field::TagApply { absent: false }).type_text("alp");
}
#[test]
fn typing_tag_apply_suggestions_zero() {
    harness(Field::TagApply { absent: true }).type_text("rzz");
}
#[test]
fn typing_tag_apply_suggestions_ime() {
    harness(Field::TagApply { absent: false }).compose_ime(&["k", "か", "かな"], "かな");
}
#[test]
fn typing_tag_apply_suggestions_unicode_text() {
    harness(Field::TagApply { absent: false }).type_text("かな本");
}

#[test]
fn typing_collection_creation_name() {
    harness(Field::CollectionName).type_text("alp");
}

#[test]
fn typing_smart_folder_rule_draft() {
    harness(Field::SmartRule).type_text("alp");
}
#[test]
fn typing_smart_folder_rule_draft_zero() {
    harness(Field::SmartRule).type_text("rzz");
}
#[test]
fn typing_smart_folder_rule_draft_ime() {
    harness(Field::SmartRule).compose_ime(&["k", "か", "かな"], "かな");
}
#[test]
fn typing_smart_folder_editor_rule() {
    harness(Field::SmartEditorRule).type_text("alp");
}
#[test]
fn typing_smart_folder_editor_rule_zero() {
    harness(Field::SmartEditorRule).type_text("rzz");
}
