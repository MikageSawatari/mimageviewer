use super::*;
use egui_kittest::kittest::NodeT;
use egui_kittest::{Harness, kittest::Queryable};

fn recipe(paths: &[PathBuf]) -> CombineRecipe {
    CombineRecipe {
        v: 1,
        options: CombineOptions::default(),
        format: CombineFormat::Jpeg { quality: 91 },
        items: paths
            .iter()
            .enumerate()
            .map(|(i, path)| CombineItemSpec {
                annotation_space: None,
                source: CombineSourceRef::File { path: path.clone() },
                label_override: Some(format!("商品{i}")),
            })
            .collect(),
    }
}

fn loaded(dir: &Path, request: u64) -> ReopenLoaded {
    let paths = [dir.join("source.png")];
    let recipe = recipe(&paths);
    let found = recipe
        .items
        .iter()
        .map(|item| {
            (
                item.clone(),
                resolver::read_source_preparation(dir, &item.source),
            )
        })
        .collect();
    ReopenLoaded {
        request,
        output: dir.join("output.jpg"),
        has_edits: true,
        recipe,
        found,
        missing: vec![],
    }
}

#[test]
fn image_combine_reopen_worker_missing_sources_and_recipe_order() {
    let temp = tempfile::tempdir().unwrap();
    let present = temp.path().join("present.png");
    let missing = temp.path().join("missing.png");
    std::fs::write(&present, b"existence only").unwrap();
    let output = temp.path().join("output.jpg");
    let expected = recipe(&[missing.clone(), present.clone()]);
    crate::image_combine::store::save_recipe(temp.path(), &output, &expected).unwrap();
    let loaded = load_reopen(temp.path(), output, true, 7, &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    assert_eq!(loaded.request, 7);
    assert_eq!(
        loaded.missing[0].source,
        CombineSourceRef::File { path: missing }
    );
    assert_eq!(
        loaded.found[0].0.source,
        CombineSourceRef::File { path: present }
    );
    assert_eq!(loaded.found[0].0.label_override.as_deref(), Some("商品1"));
    assert_eq!(loaded.recipe, expected);
    assert!(loaded.has_edits);
}

#[test]
fn image_combine_reopen_recipe_restores_output_options_labels_and_replace_default() {
    let mut app = crate::app::setup_app_for_test();
    let temp = tempfile::tempdir().unwrap();
    let mut loaded = loaded(temp.path(), 4);
    loaded.recipe.options.margin = 17;
    loaded.found[0].0.annotation_space = Some([1600, 2400]);
    let ctx = egui::Context::default();
    let Some(CombineSession::Dialog(state)) = app.image_combine_dialog_from_recipe(&ctx, loaded)
    else {
        panic!("dialog");
    };
    assert_eq!(state.request, 4);
    assert_eq!(state.options.margin, 17);
    assert_eq!(state.options.format, CombineFormat::Jpeg { quality: 91 });
    assert_eq!(state.output_dir(), temp.path());
    assert_eq!(state.name, "output");
    assert_eq!(state.items[0].spec.label_override.as_deref(), Some("商品0"));
    assert_eq!(state.items[0].spec.annotation_space, Some([1600, 2400]));
    assert!(matches!(
        &state.pending[0].1,
        PreparationTarget::Reopen {
            annotation_space: Some([1600, 2400]),
            ..
        }
    ));
    assert_eq!(
        state.replacement_path(),
        Some(temp.path().join("output.jpg").as_path())
    );
    assert!(state.reopened.as_ref().unwrap().has_edits);
    assert!(state.preset.selected_for_test().is_none());
    assert_eq!(state.pending.len(), 1);
}

#[test]
fn image_combine_reopen_missing_confirmation_and_all_missing_refuses_dialog() {
    let mut app = crate::app::setup_app_for_test();
    let temp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut input = loaded(temp.path(), 2);
    input.missing.push(CombineItemSpec {
        annotation_space: None,
        source: CombineSourceRef::File {
            path: temp.path().join("missing.png"),
        },
        label_override: None,
    });
    let Some(CombineSession::ReopenMissing(mut confirmed)) =
        app.adopt_image_combine_reopen(&ctx, input)
    else {
        panic!("confirmation");
    };
    assert!(CombineSession::ReopenMissing(Box::new(loaded(temp.path(), 2))).modal());
    confirmed.found.clear();
    assert!(app.adopt_image_combine_reopen(&ctx, *confirmed).is_none());
}

#[test]
fn image_combine_reopen_missing_confirmation_button_opens_only_found_sources() {
    let mut app = crate::app::setup_app_for_test();
    let temp = tempfile::tempdir().unwrap();
    let mut input = loaded(temp.path(), 8);
    input.missing.push(CombineItemSpec {
        annotation_space: None,
        source: CombineSourceRef::File {
            path: temp.path().join("missing.png"),
        },
        label_override: Some("外す".into()),
    });
    app.combine_session = Some(CombineSession::ReopenMissing(Box::new(input)));
    assert!(app.document_open_modal_admission_blocked());
    assert_eq!(app.modal_dialog_block_reason(), Some("image_combine"));
    let mut harness = crate::test_ui::builder()
        .with_size(egui::vec2(1093.0, 614.0))
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .build_ui_state(
            |ui, app: &mut crate::app::AppTestEnvForTest| {
                if let Some(session) = app.combine_session.take() {
                    app.combine_session = app.advance_image_combine_reopen(ui.ctx(), session);
                }
            },
            app,
        );
    harness.run();
    harness.get_by_label("見つからない画像を外して開く").click();
    harness.run();
    let Some(CombineSession::Dialog(state)) = &harness.state().combine_session else {
        panic!("dialog");
    };
    assert_eq!(state.items.len(), 1);
    assert_eq!(state.items[0].spec.label_override.as_deref(), Some("商品0"));
    assert!(
        !state
            .items
            .iter()
            .any(|item| item.spec.label_override.as_deref() == Some("外す"))
    );
}

#[test]
fn image_combine_reopen_replace_fixed_target_preserves_quality_and_refuses_source() {
    let ctx = egui::Context::default();
    let mut state = super::super::fixture_state(&ctx, false);
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("original.jpg");
    state.reopened = Some(ReopenedOutput {
        path: output.clone(),
        format: CombineFormat::Jpeg { quality: 91 },
        replace: true,
        has_edits: true,
    });
    state.options.format = CombineFormat::Png;
    state.name = "changed".into();
    state.options.destination = Destination::FirstImageFolder;
    state.enforce_replacement_target();
    assert_eq!(state.name, "original");
    assert_eq!(state.output_dir(), temp.path());
    assert_eq!(state.options.format, CombineFormat::Jpeg { quality: 91 });
    state.options.format = CombineFormat::Jpeg { quality: 77 };
    state.enforce_replacement_target();
    assert_eq!(state.options.format, CombineFormat::Jpeg { quality: 77 });
    state.items[0].spec.source = CombineSourceRef::File { path: output };
    assert!(state.replacement_is_source());
    assert!(
        state
            .output_validation()
            .unwrap_err()
            .contains("素材に含まれる")
    );
    state.reopened.as_mut().unwrap().replace = false;
    assert!(state.output_validation().is_ok());
    state.options.format = CombineFormat::Png;
    state.enforce_replacement_target();
    assert_eq!(state.options.format, CombineFormat::Png);
}

#[test]
fn image_combine_reopen_replace_window_locks_name_and_preserves_format() {
    let ctx = egui::Context::default();
    let mut state = super::super::fixture_state(&ctx, false);
    let temp = tempfile::tempdir().unwrap();
    state.reopened = Some(ReopenedOutput {
        path: temp.path().join("output.jpg"),
        format: CombineFormat::Jpeg { quality: 95 },
        replace: true,
        has_edits: true,
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(1093.0, 614.0))
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .build_ui_state(
            |ui, state: &mut CombineDialogState| {
                super::super::draw_window(ui.ctx(), state, &[]);
            },
            state,
        );
    h.run();
    assert!(
        h.get_by_role_and_label(egui::accesskit::Role::TextInput, "名前")
            .accesskit_node()
            .is_disabled()
    );
    // The warning and target controls share the scrolling output section.
    assert!(h.state().reopened.as_ref().unwrap().has_edits);
    assert_eq!(
        h.state().options.format,
        CombineFormat::Jpeg { quality: 95 }
    );
}

#[test]
fn image_combine_reopen_loading_modal_escape_drops_late_result_and_reentry() {
    let mut app = crate::app::setup_app_for_test();
    app.combine_index.set_keys_for_test(Default::default());
    app.combine_request_sequence = 12;
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    app.combine_session = Some(CombineSession::ReopenLoading(Box::new(ReopenRequest {
        request: 12,
        cancel: cancel.clone(),
        rx,
    })));
    assert_eq!(app.modal_dialog_block_reason(), Some("image_combine"));
    assert!(app.document_open_modal_admission_blocked());
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    app.open_image_combine_dialog(&ctx);
    assert!(matches!(
        app.combine_session,
        Some(CombineSession::ReopenLoading(_))
    ));
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1093.0, 614.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: Some(egui::Key::Escape),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ui| app.draw_image_combine_dialog(ui.ctx()),
    )
    .drop_without_applying_deltas();
    assert!(app.combine_session.is_none());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(tx.send((12, Ok(None))).is_err());
}

#[test]
fn image_combine_reopen_stale_request_cannot_adopt_or_reload_index() {
    let mut app = crate::app::setup_app_for_test();
    let ctx = egui::Context::default();
    app.combine_request_sequence = 2;
    let (tx, rx) = mpsc::channel();
    tx.send((1, Ok(None))).unwrap();
    let mut pending = Some(ReopenRequest {
        request: 1,
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
    });
    let mut result = None;
    ctx.run_ui(Default::default(), |ui| {
        if let Some(pending) = pending.take() {
            result = app.advance_image_combine_reopen(
                ui.ctx(),
                CombineSession::ReopenLoading(Box::new(pending)),
            );
        }
    })
    .drop_without_applying_deltas();
    assert!(result.is_none());
}

#[test]
fn image_combine_reopen_missing_row_marks_loaded_index_stale() {
    let mut app = crate::app::setup_app_for_test();
    let temp = tempfile::tempdir().unwrap();
    let key = crate::adjustment_db::normalize_path(&temp.path().join("output.png"));
    app.combine_index.set_keys_for_test([key.clone()].into());
    app.combine_request_sequence = 3;
    let (tx, rx) = mpsc::channel();
    tx.send((3, Ok(None))).unwrap();
    let ctx = egui::Context::default();
    let mut pending = Some(ReopenRequest {
        request: 3,
        cancel: Arc::new(AtomicBool::new(false)),
        rx,
    });
    ctx.run_ui(Default::default(), |ui| {
        if let Some(pending) = pending.take() {
            assert!(
                app.advance_image_combine_reopen(
                    ui.ctx(),
                    CombineSession::ReopenLoading(Box::new(pending))
                )
                .is_none()
            );
        }
    })
    .drop_without_applying_deltas();
    // Polling against an empty disposable store must remove the old key.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.combine_index.contains_or_unloaded(&key) {
        app.combine_index.poll(temp.path(), &ctx);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn image_combine_store_effects_reload_index_even_when_content_identity_disabled() {
    let mut app = crate::app::setup_app_for_test();
    app.settings.edit_restore_prompt_enabled = false;
    app.combine_index.set_keys_for_test(Default::default());
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("copy.jpg");
    crate::image_combine::store::save_recipe(
        temp.path(),
        &output,
        &recipe(&[temp.path().join("source.png")]),
    )
    .unwrap();
    let mut effects = crate::rename_key_migration::StoreMutationEffects::default();
    let descriptor = crate::rename_key_migration::STORES
        .iter()
        .find(|descriptor| descriptor.file == "combine.db")
        .unwrap();
    effects.record_rows_changed(descriptor, 1);
    app.apply_content_identity_store_mutations(effects);
    let key = crate::adjustment_db::normalize_path(&output);
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !app.combine_index.contains_or_unloaded(&key) {
        app.combine_index.poll(temp.path(), &ctx);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
}
