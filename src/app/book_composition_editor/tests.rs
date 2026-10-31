#![cfg(test)]

use super::*;
use crate::app::book_materialization;
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};
use crate::spread_shift::{ShiftOperation, SpreadShiftPageRef, SpreadShiftService};
use std::{io::Write, sync::Arc, time::Duration};

const WAIT: Duration = Duration::from_secs(15);

fn section1373_receipt(app: &AppTestEnv) -> crate::book_append_undo::BookAppendUndoReceipt {
    let root = app.tmp.path().join(format!(
        "composition-append-books-{}",
        app.book_append_undo_revision,
    ));
    let folder = root.join("book");
    std::fs::create_dir_all(&folder).unwrap();
    let mut builder =
        crate::book_append_undo::AppendReceiptBuilder::begin(&root, &folder, "book".into())
            .unwrap();
    let page = folder.join("0001_page.jpg");
    std::fs::write(&page, b"generated page").unwrap();
    builder.record(&page).unwrap();
    crate::book_append_undo::BookAppendUndoReceipt {
        files: builder.finish().unwrap(),
        operation_id: 1373,
        source_context: app.projected_viewer_context_id(),
        accepted_revision: app.book_append_undo_revision,
    }
}

#[test]
fn section1373_composition_draft_history_invalidates_only_accepted_changes() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    start_editing(&mut f.app, &ctx);
    let before = draft_record(&f);
    f.app.book_append_undo_receipt = Some(section1373_receipt(&f.app));
    let revision = f.app.book_append_undo_revision;
    for action in [
        ui::EditorUiAction::Undo,
        ui::EditorUiAction::Redo,
        ui::EditorUiAction::SetHidden {
            page: usize::MAX,
            hidden: true,
        },
    ] {
        f.app.apply_editor_ui_action(&ctx, action);
        assert_eq!(f.app.book_append_undo_revision, revision);
        assert!(f.app.book_append_undo_receipt.is_some());
        assert_eq!(draft_record(&f), before);
    }
    f.app.apply_editor_ui_action(
        &ctx,
        ui::EditorUiAction::SetHidden {
            page: 1,
            hidden: true,
        },
    );
    let after = draft_record(&f);
    assert_ne!(after, before);
    assert!(f.app.book_append_undo_revision > revision);
    assert!(f.app.book_append_undo_receipt.is_none());
    for (action, expected) in [
        (ui::EditorUiAction::Undo, before),
        (ui::EditorUiAction::Redo, after),
    ] {
        f.app.book_append_undo_receipt = Some(section1373_receipt(&f.app));
        let revision = f.app.book_append_undo_revision;
        f.app.apply_editor_ui_action(&ctx, action);
        assert!(f.app.book_append_undo_revision > revision);
        assert!(f.app.book_append_undo_receipt.is_none());
        assert_eq!(draft_record(&f), expected);
    }
}

#[test]
fn section1373_composition_direct_cover_invalidates_only_accepted_change() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    start_editing(&mut f.app, &ctx);
    let enabled = !draft_record(&f).cover_enabled;
    f.app.book_append_undo_receipt = Some(section1373_receipt(&f.app));
    let revision = f.app.book_append_undo_revision;
    f.app
        .apply_editor_ui_action(&ctx, ui::EditorUiAction::SetCover(enabled));
    assert_eq!(draft_record(&f).cover_enabled, enabled);
    assert!(f.app.book_append_undo_revision > revision);
    assert!(f.app.book_append_undo_receipt.is_none());
    f.app.book_append_undo_receipt = Some(section1373_receipt(&f.app));
    let revision = f.app.book_append_undo_revision;
    f.app
        .apply_editor_ui_action(&ctx, ui::EditorUiAction::SetCover(enabled));
    assert_eq!(f.app.book_append_undo_revision, revision);
    assert!(f.app.book_append_undo_receipt.is_some());
}

#[test]
fn section1373_composition_save_remove_admission_rejects_busy_without_mutation() {
    for remove in [false, true] {
        for busy in [0, 1, 2, 3, 4] {
            let mut f = fixture();
            let ctx = egui::Context::default();
            let handle = remote(&mut f.app);
            start_editing(&mut f.app, &ctx);
            let before = draft_record(&f);
            let receipt = section1373_receipt(&f.app);
            f.app.book_append_undo_receipt = Some(receipt.clone());
            if busy == 1 {
                let (_tx, rx) = mpsc::channel();
                f.app.book_op_pending = Some(crate::books::BookOpPending {
                    rx,
                    intent: crate::books::BookOpIntent::UndoAppend { receipt },
                });
            } else if busy == 2 {
                acquire(&handle);
            } else if busy == 3 || busy == 4 {
                let (_tx, rx) = mpsc::channel();
                let intent = if busy == 3 {
                    crate::books::BookOpIntent::SourceMutation
                } else {
                    crate::books::BookOpIntent::Append {
                        operation_id: receipt.operation_id,
                        source_context: receipt.source_context,
                        accepted_revision: receipt.accepted_revision,
                        target_folder: receipt.files.folder.clone(),
                        observations: Default::default(),
                    }
                };
                f.app.book_op_pending = Some(crate::books::BookOpPending { rx, intent });
            }
            let revision = f.app.book_append_undo_revision;
            f.app.save_local_composition_editor(&ctx, remove);
            if busy == 0 || busy == 3 || busy == 4 {
                assert!(f.app.book_append_undo_revision > revision);
                assert!(f.app.book_append_undo_receipt.is_none());
                assert!(f.app.composition_editor_writes_pending());
                assert!(buffer_save_completion(&mut f.app));
                f.app.poll_composition_editor_operations(&ctx);
            } else {
                assert_eq!(f.app.book_append_undo_revision, revision);
                assert!(f.app.book_append_undo_receipt.is_some());
                assert!(!f.app.composition_editor_writes_pending());
                assert_eq!(draft_record(&f), before);
                if busy == 1 {
                    f.app.apply_editor_ui_action(
                        &ctx,
                        ui::EditorUiAction::SetHidden {
                            page: 1,
                            hidden: true,
                        },
                    );
                    assert_eq!(f.app.book_append_undo_revision, revision);
                    assert_eq!(draft_record(&f), before);
                }
            }
        }
    }
}

#[test]
fn section1373_composition_sent_ticket_blocks_append_undo_after_editor_close() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    start_editing(&mut f.app, &ctx);
    let owner = f.app.composition_editor_operations.owner.clone().unwrap();
    let (_tx, receiver) = mpsc::channel();
    f.app
        .composition_editor_operations
        .tickets
        .push(SentSaveTicket { owner, receiver });
    f.app
        .close_local_composition_editor(CompositionEditorCloseReason::Cancel);
    assert!(!f.app.composition_editor_modal_active());
    f.app.book_append_undo_receipt = Some(section1373_receipt(&f.app));
    let revision = f.app.book_append_undo_revision;
    assert!(f.app.try_undo_book_append(&ctx));
    assert_eq!(f.app.book_append_undo_revision, revision);
    assert!(f.app.book_append_undo_receipt.is_some());
    assert!(!f.app.book_append_undo_in_progress());
}

#[test]
fn image_folder_editor_entry_only_guides_and_preserves_existing_reader() {
    let mut f = fixture();
    let root = f.app.tmp.path().join("image-folder");
    std::fs::create_dir(&root).unwrap();
    let pages = ["a.png", "b.png"].map(|name| root.join(name));
    for page in &pages {
        image::RgbaImage::new(8, 16).save(page).unwrap();
    }
    f.app.settings.auto_fullscreen_zip_pdf = true;
    f.app.settings.auto_fullscreen_image_folders = true;
    f.app.current_folder = Some(root.clone());
    f.app.zip_nav = None;
    f.app.items = pages
        .into_iter()
        .map(crate::grid_item::GridItem::Image)
        .collect();
    f.app.projection_recipe = book_materialization::ProjectionRecipe::for_physical(
        &f.app.settings,
        &root,
        &root,
        &f.app.items,
    );
    assert!(matches!(
        &f.app.projection_recipe,
        book_materialization::ProjectionRecipe::Book(book_materialization::BookScope::ImageFolder(
            _
        ))
    ));
    f.app.book_composition = ViewerBookComposition::Automatic;
    f.app.fullscreen_idx = Some(0);
    let rows = f.app.items.clone();
    let settings = serde_json::to_value(&f.app.settings).unwrap();
    // Startup publication is a separate worker boundary. Finish it explicitly
    // before asserting the guide does not publish another snapshot.
    f.service
        .prepare(f.scope.clone(), f.domain.clone())
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    let snapshot = f.service.snapshot();
    let ctx = egui::Context::default();
    for mode in [
        crate::settings::SpreadMode::Single,
        crate::settings::SpreadMode::Rtl,
        crate::settings::SpreadMode::SplitRtl,
    ] {
        f.app.spread_mode = mode;
        assert!(f.app.composition_editor_available());
        assert!(f.app.start_local_composition_editor(&ctx));
        assert!(f.app.image_folder_zip_operation.is_some());
        assert!(f.app.common_modal_dialog_open());
        // The guide now offers conversion; dismissing it still leaves the reader intact.
        f.app.image_folder_zip_operation = None;
        assert!(matches!(
            f.app.composition_editor,
            CompositionEditorState::Closed
        ));
        assert!(!f.app.composition_editor_modal_active());
        assert!(!f.app.composition_editor_writes_pending());
        assert!(f.app.composition_scope_domain().is_none());
        assert_eq!(f.app.items, rows);
        assert_eq!(f.app.current_folder.as_ref(), Some(&root));
        assert_eq!(f.app.fullscreen_idx, Some(0));
        assert_eq!(f.app.spread_mode, mode);
        assert_eq!(serde_json::to_value(&f.app.settings).unwrap(), settings);
        assert!(Arc::ptr_eq(&snapshot, &f.service.snapshot()));
        assert!(f.service.drain_changed_scope_keys().is_empty());
    }
}

fn editor_harness(
    f: Fixture,
) -> (
    std::rc::Rc<std::cell::RefCell<Fixture>>,
    egui_kittest::Harness<'static>,
) {
    let fixture = std::rc::Rc::new(std::cell::RefCell::new(f));
    let renderer = fixture.clone();
    let mut fonts_ready = false;
    let h = crate::test_ui::builder()
        .with_size(egui::vec2(1093.0, 614.0))
        .build_ui(move |root_ui| {
            let ctx = &root_ui.ctx().clone();
            ctx.global_style_mut(|style| {
                style.scroll_animation = egui::style::ScrollAnimation::none()
            });
            if !fonts_ready {
                crate::ui_fonts::configure_fonts(ctx);
                crate::ime_focus::install_ime_input_policy(ctx);
                fonts_ready = true;
                ctx.request_repaint();
                return;
            }
            let mut f = renderer.borrow_mut();
            f.app.update_ime_state(ctx);
            f.app.show_local_composition_editor(ctx);
        });
    (fixture, h)
}

#[cfg(windows)]
#[test]
fn egui037_continuation_context_state_real_logic_receipt_keeps_sibling_ui_unadopted() {
    use crate::app::context_state_tests::Fixture as ContextFixture;
    let ContextFixture {
        mut env,
        main,
        other,
        old_stamp,
        ..
    } = ContextFixture::new();
    env.book_resume_writer = Some(
        crate::book_resume_db::BookResumeWriter::spawn_at(
            env.tmp.path().join("real-logic-receipt.db"),
        )
        .unwrap(),
    );
    let owner = CompositionEditorOwner {
        context_id: other,
        scope: BookScopeKey::document(&env.tmp.path().join("authoritative.cbz")).unwrap(),
        request_id: 702,
    };
    env.composition_editor_operations.owner = Some(owner.clone());
    let path = owner.scope.logical_root.clone();
    let (worker, receiver) = mpsc::channel();
    env.composition_editor_operations
        .tickets
        .push(SentSaveTicket { owner, receiver });
    worker
        .send(SaveCompletion {
            status: Ok(Some(ResumeCommit {
                key: path.clone(),
                position: ReadingPosition {
                    page: 2,
                    identity: None,
                },
                meter: ReadingMeterValue::new(2, 4).unwrap(),
            })),
        })
        .unwrap();
    let ctx = egui::Context::default();
    let mut integration = eframe::HeadlessEpiIntegration::new(ctx.clone());
    let generation = env.items_generation;
    let publications = env.rating_publication_count;
    App::start_rating_boundary_trace_for_test();
    for _ in 0..3 {
        integration.update_logic_only(&mut env.app, egui::RawInput::default());
        assert_eq!(ctx.cumulative_frame_nr(), 0);
        assert_eq!(env.projected_viewer_context_id(), main);
        assert_eq!(env.items_generation, generation);
        assert_eq!(env.rating_publication_count, publications);
        for id in [main, other] {
            env.with_viewer_context_state(id, |app| {
                ContextFixture::assert_pending(app, id, if id == main { 4 } else { 7 }, old_stamp);
                assert_eq!(
                    matches!(app.composition_editor, CompositionEditorState::Reloading),
                    id == other
                );
            })
            .unwrap();
        }
        assert!(App::take_rating_boundary_trace_for_test().is_empty());
        assert!(!env.composition_editor_writes_pending());
        assert_eq!(
            env.book_resume_writer
                .as_ref()
                .unwrap()
                .watermarks()
                .admitted,
            1
        );
    }
    App::stop_rating_boundary_trace_for_test();
    let reply = env
        .book_resume_writer
        .as_ref()
        .unwrap()
        .read_position_after(&crate::path_key::normalize(&path), 1)
        .recv_timeout(std::time::Duration::from_secs(15))
        .unwrap();
    assert_eq!(reply.position.unwrap().page, 2);
}

fn draft_record(f: &Fixture) -> BookComposition {
    let CompositionEditorState::Editing(session) = &f.app.composition_editor else {
        panic!("Editing");
    };
    session.prepared.as_ref().unwrap().draft.record().clone()
}

#[test]
fn real_page_directive_is_immediate_and_one_undo_preserves_prior_draft() {
    use egui_kittest::kittest::Queryable;
    let mut f = fixture();
    let ctx = egui::Context::default();
    start_editing(&mut f.app, &ctx);
    f.app.apply_editor_ui_action(
        &ctx,
        ui::EditorUiAction::SetDirection(ReadingDirection::Rtl),
    );
    finish_editor_results(&mut f.app);
    let before = draft_record(&f);
    let CompositionEditorState::Editing(session) = &mut f.app.composition_editor else {
        panic!("Editing");
    };
    session.ui.selected_pages = vec![1];
    session.refresh_view();
    let (f, mut h) = editor_harness(f);
    h.run();
    h.get_by_label("単独で表示").click();
    h.step();
    let record = draft_record(&f.borrow());
    assert_eq!(
        record.page_override(&record.page_order[1]),
        Some(PageOverride::Alone)
    );
    assert_eq!(record.direction, before.direction);
    assert!(h.query_by_label("変更する").is_none());
    h.run(); // Publish the updated UI tree before clicking the next control.
    h.get_by_label("元に戻す").click();
    h.step();
    assert_eq!(draft_record(&f.borrow()), before);
}

#[test]
fn real_escape_cancels_direct_draft_without_a_second_confirmation_phase() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    start_editing(&mut f.app, &ctx);
    f.app.apply_editor_ui_action(
        &ctx,
        ui::EditorUiAction::SetOverride {
            page: 1,
            directive: Some(PageOverride::BlankOpposite {
                page_side: PageSide::Leading,
            }),
        },
    );
    assert!(
        draft_record(&f)
            .page_override(&draft_record(&f).page_order[1])
            .is_some()
    );
    let (f, mut h) = editor_harness(f);
    h.run();
    h.key_press(egui::Key::Escape);
    h.step();
    assert!(!f.borrow().app.composition_editor_modal_active());
    assert!(stored(&f.borrow()).is_none());
}

#[test]
#[expect(
    deprecated,
    reason = "Exercise retained IME compatibility notifications and cancellation ordering"
)]
fn confirmation_keyboard_uses_ime_helpers_and_escape_only_dismisses_the_front_dialog() {
    for confirmation in [
        ui::EditorConfirmation::Remove,
        ui::EditorConfirmation::Reload,
    ] {
        let mut f = fixture();
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        crate::ime_focus::install_ime_input_policy(&ctx);
        start_editing(&mut f.app, &ctx);
        let before = draft_record(&f);
        let CompositionEditorState::Editing(session) = &mut f.app.composition_editor else {
            panic!("editing");
        };
        session.ui.confirmation = confirmation;
        let frame = |f: &mut Fixture, events| {
            ctx.run_pass(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1093.0, 614.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    f.app.update_ime_state(ctx);
                    f.app.show_local_composition_editor(ctx);
                },
            )
            .drop_without_applying_deltas();
        };
        frame(&mut f, vec![]);
        frame(&mut f, vec![]);
        for key in [egui::Key::Enter, egui::Key::Escape] {
            frame(
                &mut f,
                vec![
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit {
                        text: "未確定".into(),
                        active_range_chars: None,
                    }),
                    egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            assert!(f.app.ime_input_active(&ctx));
            assert!(
                matches!(&f.app.composition_editor, CompositionEditorState::Editing(session) if session.ui.confirmation == confirmation)
            );
            assert_eq!(draft_record(&f), before);
            frame(
                &mut f,
                vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        frame(
            &mut f,
            vec![egui::Event::Ime(egui::ImeEvent::Commit("".into()))],
        );
        frame(
            &mut f,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(
            matches!(&f.app.composition_editor, CompositionEditorState::Editing(session) if session.ui.confirmation == ui::EditorConfirmation::None)
        );
        assert_eq!(draft_record(&f), before);
        // Qualified Enter confirms exactly the current choice.
        frame(
            &mut f,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let CompositionEditorState::Editing(session) = &mut f.app.composition_editor else {
            panic!("editing");
        };
        session.ui.confirmation = confirmation;
        let request = f
            .app
            .composition_editor_operations
            .owner
            .as_ref()
            .unwrap()
            .request_id;
        frame(&mut f, vec![]);
        frame(&mut f, vec![]);
        frame(
            &mut f,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        if confirmation == ui::EditorConfirmation::Remove {
            assert!(f.app.composition_editor_writes_pending());
        } else {
            assert_ne!(
                f.app
                    .composition_editor_operations
                    .owner
                    .as_ref()
                    .unwrap()
                    .request_id,
                request
            );
        }
    }
}

pub(super) fn finish_editor_results(app: &mut App) {
    let CompositionEditorState::Editing(session) = &mut app.composition_editor else {
        return;
    };
    let generation = session.prepared.as_ref().unwrap().draft.generation();
    while session.prepared.as_ref().unwrap().result.generation != generation {
        let (received, result) = session
            .result_worker
            .as_ref()
            .unwrap()
            .receiver
            .recv_timeout(WAIT)
            .unwrap();
        if received == generation {
            session.prepared.as_mut().unwrap().result = result.unwrap();
        }
    }
    session.refresh_view();
}

#[test]
fn stale_result_cannot_replace_latest_draft_selection_or_enable_save() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    start_editing(&mut f.app, &ctx);
    let CompositionEditorState::Editing(session) = &mut f.app.composition_editor else {
        panic!("Editing");
    };
    let (sender, receiver) = mpsc::channel();
    session.result_worker.as_mut().unwrap().receiver = receiver;
    session.ui.selected_pages = vec![1];
    let stale = session.prepared.as_ref().unwrap().result.clone();
    f.app.apply_editor_ui_action(
        &ctx,
        ui::EditorUiAction::MovePageStep {
            page: 1,
            forward: true,
        },
    );
    let CompositionEditorState::Editing(session) = &mut f.app.composition_editor else {
        panic!("Editing");
    };
    let prepared = session.prepared.as_ref().unwrap();
    let generation = prepared.draft.generation();
    let current =
        preparation::derive_result(&prepared.result_source, prepared.draft.record(), generation)
            .unwrap();
    sender.send((stale.generation, Ok(stale.clone()))).unwrap();
    session.poll_results();
    assert_eq!(session.prepared.as_ref().unwrap().result, stale);
    assert_eq!(session.ui.selected_pages, vec![1]);
    assert!(session.view.results_updating);
    assert!(!session.view.can_save);
    sender.send((generation, Ok(current.clone()))).unwrap();
    session.poll_results();
    assert_eq!(session.prepared.as_ref().unwrap().result, current);
    assert!(session.view.can_save);
    assert_eq!(session.ui.selected_pages, vec![1]);
}

#[test]
fn hidden_cover_is_not_presented_or_requested_while_latest_result_is_delayed() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    start_editing(&mut f.app, &ctx);
    let CompositionEditorState::Editing(session) = &mut f.app.composition_editor else {
        panic!("Editing");
    };
    let mut record = session.prepared.as_ref().unwrap().draft.record().clone();
    record.cover_enabled = true;
    session.input.rules.final_cover = true;
    session.prepared = Ok(preparation::prepare(&session.input, Some(&record)).unwrap());
    let cover_id = session
        .pages
        .iter()
        .position(|p| p.matching_key() == record.page_order[0].matching_key())
        .unwrap();
    session.refresh_view();
    assert!(session.view.units.iter().any(|unit| matches!(unit,
        ui::EditorUnitView::Single { companion: ui::EditorCompanionView::Cover(id), .. } if *id == cover_id
    )), "fixture must start with a real derived final-cover supplement");
    // Hold the session's result receiver. The original worker may finish, but
    // its reply cannot reach this owner until the explicit latest reply below.
    let (sender, receiver) = mpsc::channel();
    session.result_worker.as_mut().unwrap().receiver = receiver;
    f.app.apply_editor_ui_action(
        &ctx,
        ui::EditorUiAction::SetHidden {
            page: cover_id,
            hidden: true,
        },
    );
    let CompositionEditorState::Editing(session) = &mut f.app.composition_editor else {
        panic!("Editing");
    };
    session.poll_results();
    assert!(session.view.results_updating);
    assert!(session.view.pages[cover_id].hidden);
    assert_eq!(session.view.cover, None);
    assert!(session.view.units.iter().all(|unit| match unit {
        ui::EditorUnitView::Pair { first, second } => *first != cover_id && *second != cover_id,
        ui::EditorUnitView::Single {
            page, companion, ..
        } =>
            *page != cover_id
                && !matches!(companion, ui::EditorCompanionView::Cover(id) if *id == cover_id),
    }));
    let mut actions = Vec::new();
    for _ in 0..3 {
        ctx.run_pass(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(620.0, 440.0),
                )),
                ..Default::default()
            },
            |ctx| {
                ctx.root_ui(|ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        // The result tab isolates right-side thumbnail demand. Hidden
                        // pages remain valid thumbnail sources in the left list.
                        session.ui.panel = ui::EditorPanel::Units;
                        actions = ui::render(ui, &session.view, &mut session.ui);
                    })
                });
            },
        )
        .drop_without_applying_deltas();
        assert!(
            actions.iter().all(|action| !matches!(action,
                ui::EditorUiAction::RequestThumbnails(ids) if ids.contains(&cover_id)
            )),
            "a hidden auxiliary cover must not request a right-side thumbnail"
        );
    }
    let prepared = session.prepared.as_ref().unwrap();
    let generation = prepared.draft.generation();
    let current =
        preparation::derive_result(&prepared.result_source, prepared.draft.record(), generation)
            .unwrap();
    sender.send((generation, Ok(current))).unwrap();
    session.poll_results();
    assert!(!session.view.results_updating);
    assert_ne!(session.view.cover, Some(cover_id));
}

#[test]
fn modal_editor_disables_real_backing_menus_and_pauses_slideshow_handlers() {
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut f = fixture();
    let ctx = egui::Context::default();
    f.app.fullscreen_idx = Some(0);
    f.app.slideshow_playing = true;
    start_editing(&mut f.app, &ctx);
    let generation = f.app.items_generation;
    let settings = f.app.settings.clone();
    f.app.advance_slideshow(&ctx, 0);
    assert_eq!(f.app.fullscreen_idx, Some(0));
    assert_eq!(f.app.items_generation, generation);
    assert!(
        f.app.slideshow_playing,
        "modal pauses rather than stopping playback"
    );
    let fixture = std::rc::Rc::new(std::cell::RefCell::new(f));
    let renderer = fixture.clone();
    let mut fonts_ready = false;
    let mut h = crate::test_ui::builder()
        .with_size(egui::vec2(1093.0, 614.0))
        .build_ui(move |root_ui| {
            let ctx = &root_ui.ctx().clone();
            if !fonts_ready {
                crate::ui_fonts::configure_fonts(ctx);
                fonts_ready = true;
                ctx.request_repaint();
                return;
            }
            renderer.borrow_mut().app.render_menubar(root_ui);
        });
    h.run();
    assert!(h.get_by_label("設定").accesskit_node().is_disabled());
    assert!(h.get_by_label("製本").accesskit_node().is_disabled());
    assert!(fixture.borrow().app.composition_editor_modal_active());
    assert_eq!(
        serde_json::to_value(&fixture.borrow().app.settings).unwrap(),
        serde_json::to_value(&settings).unwrap()
    );
}

struct Fixture {
    app: AppTestEnv,
    service: BookCompositionService,
    composition_process: Option<BookCompositionProcessOwner>,
    shift: SpreadShiftService,
    shift_process: Option<crate::spread_shift::SpreadShiftProcessOwner>,
    scope: BookScopeKey,
    domain: PageDomain,
    tree: Arc<crate::zip_tree::ZipTree>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(process) = self.composition_process.take() {
            process.shutdown_and_join();
        }
        if let Some(process) = self.shift_process.take() {
            process.shutdown_and_join();
        }
    }
}

fn fixture() -> Fixture {
    fixture_with_pages(&["a.png", "b.png", "c.png", "d.png"].map(str::to_string))
}

fn fixture_with_pages(names: &[String]) -> Fixture {
    let mut app = setup_app();
    app.book_resume_writer = None;
    let root = app.tmp.path().join("editor-book.zip");
    let mut archive = zip::ZipWriter::new(std::fs::File::create(&root).unwrap());
    for name in names {
        archive
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(8, 16))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        archive.write_all(bytes.get_ref()).unwrap();
    }
    archive.finish().unwrap();
    let entries =
        crate::zip_loader::enumerate_image_entries_detailed_readonly_with_cancel(&root, None)
            .unwrap()
            .entries;
    let tree = Arc::new(crate::zip_tree::ZipTree::build(root.clone(), entries));
    install_archive(&mut app, &tree);
    let scope = BookScopeKey::archive(&root, &[]).unwrap();
    let domain = PageDomain::ArchiveLevel {
        prefix: String::new(),
    };
    let (service, composition_process) =
        BookCompositionService::spawn_at(app.tmp.path().into(), None).unwrap();
    app.install_book_composition_runtime(service.clone());
    let (shift, shift_process) = SpreadShiftService::spawn_at(app.tmp.path().into(), None).unwrap();
    let cutoff = shift
        .set_shift(
            scope.clone(),
            SpreadShiftPageRef::ArchiveEntry(names[1].clone()),
            ShiftOperation {
                context_id: app.projected_viewer_context_id(),
                items_generation: app.items_generation,
                operation_sequence: 1,
            },
        )
        .unwrap();
    assert!(
        shift
            .read_after(scope.clone(), cutoff)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .is_ok()
    );
    // This seed is an already completed operation, not an unharvested UI write. Remote's
    // existing book-I/O barrier includes completed receipts until their owner consumes them.
    let seed_receipts = shift.drain_results();
    assert_eq!(seed_receipts.len(), 1);
    assert_eq!(seed_receipts[0].sequence, cutoff);
    assert!(seed_receipts[0].status.is_ok());
    shift.drain_changed_scope_keys();
    app.spread_shift_service = Some(shift.clone());
    app.spread_shift.scope = Some(scope.clone());
    app.spread_shift.state = crate::app::spread_shift_owner::ViewerShift::Book(
        crate::spread_shift::SpreadShiftState::Adjusted {
            anchor_ref: SpreadShiftPageRef::ArchiveEntry(names[1].clone()),
            resolution: crate::spread_shift::ShiftResolution::Resolved(1),
            persistence: crate::spread_shift::ShiftPersistence::Saved(cutoff),
        },
    );
    Fixture {
        app,
        service,
        composition_process: Some(composition_process),
        shift,
        shift_process: Some(shift_process),
        scope,
        domain,
        tree,
    }
}

fn install_archive(app: &mut App, tree: &Arc<crate::zip_tree::ZipTree>) {
    app.current_folder = Some(tree.zip_path.clone());
    app.zip_nav = Some(crate::zip_tree::ZipNavState::new(Arc::clone(tree)));
    app.projection_recipe =
        book_materialization::ProjectionRecipe::Book(book_materialization::BookScope::Archive {
            logical_root: tree.zip_path.clone(),
            prefix: vec![],
        });
    let (items, metadata) = book_materialization::materialize_archive_level(
        tree,
        &[],
        crate::settings::SortOrder::FileName,
        Some(&app.settings.grid_display_order),
    );
    app.items = items;
    app.thumbnails = vec![crate::grid_item::ThumbnailState::Pending; app.items.len()];
    app.image_metas = metadata;
    app.visible_indices = (0..app.items.len()).collect();
    app.page_identity_index = Arc::new(
        crate::app::file_type_resume::PreparedPageIdentityIndex::prepare(
            &app.items,
            Some(&tree.zip_path),
            app.settings.file_type_visibility.clone(),
        ),
    );
    app.book_composition = ViewerBookComposition::Automatic;
    app.fullscreen_idx = None;
    app.spread_mode = crate::settings::SpreadMode::Ltr;
}

/// Wait for exactly the admitted worker and deliver through the production poll.
fn deliver_prepare(app: &mut App, ctx: &egui::Context) {
    let CompositionEditorState::Preparing { input, receiver } =
        std::mem::take(&mut app.composition_editor)
    else {
        panic!("expected Preparing");
    };
    let completion = receiver
        .recv_timeout(WAIT)
        .expect("editor prepare completion");
    let (tx, rx) = mpsc::channel();
    tx.send(completion).unwrap();
    app.composition_editor = CompositionEditorState::Preparing {
        input,
        receiver: rx,
    };
    app.poll_composition_editor_operations(ctx);
}

fn finish_prepare(app: &mut App, ctx: &egui::Context) {
    deliver_prepare(app, ctx);
    let CompositionEditorState::Editing(session) = &app.composition_editor else {
        panic!("expected Editing");
    };
    assert!(session.prepared.is_ok(), "{:?}", session.prepared);
}

fn finish_projection(app: &mut App, ctx: &egui::Context) {
    let ViewerBookComposition::Projecting {
        scope,
        token,
        record,
        source,
        generation,
        origin,
        receiver,
    } = std::mem::take(&mut app.book_composition)
    else {
        panic!("expected Projecting");
    };
    let completion = receiver.recv_timeout(WAIT).expect("projection completion");
    assert!(completion.is_ok());
    let (tx, rx) = mpsc::channel();
    tx.send(completion).unwrap();
    app.book_composition = ViewerBookComposition::Projecting {
        scope,
        token,
        record,
        source,
        generation,
        origin,
        receiver: rx,
    };
    app.poll_book_composition_read(ctx);
}

fn advance_read_to_projection(app: &mut App, ctx: &egui::Context) {
    if matches!(
        app.book_composition,
        ViewerBookComposition::Preparing { .. }
    ) {
        let ViewerBookComposition::Preparing {
            scope,
            receiver,
            origin,
        } = std::mem::take(&mut app.book_composition)
        else {
            unreachable!();
        };
        let completion = receiver
            .recv_timeout(WAIT)
            .expect("composition read completion");
        let (tx, rx) = mpsc::channel();
        tx.send(completion).unwrap();
        app.book_composition = ViewerBookComposition::Preparing {
            scope,
            receiver: rx,
            origin,
        };
        app.poll_book_composition_read(ctx);
    }
    assert!(matches!(
        app.book_composition,
        ViewerBookComposition::Projecting { .. }
    ));
}

fn finish_read_projection(app: &mut App, ctx: &egui::Context) {
    advance_read_to_projection(app, ctx);
    finish_projection(app, ctx);
}

fn accept_automatic(app: &mut App, service: &BookCompositionService, scope: &BookScopeKey) {
    app.book_composition = ViewerBookComposition::Accepted {
        scope: scope.clone(),
        token: service.snapshot().token_for(scope),
        record: None,
        layout: CompositionProjectionOutcome::Automatic,
        source: app.composition_source(),
        generation: app.items_generation,
        cause: super::super::book_composition_runtime::ProjectionAdoptionCause::Normal,
    };
}

fn fullscreen_identity(app: &App) -> PageIdentity {
    let index = app.fullscreen_idx.expect("viewer remains fullscreen");
    PageIdentity::for_item(&app.items[index], app.archive_source_override.as_deref())
        .expect("fullscreen image identity")
}

fn record_json(f: &Fixture) -> String {
    let connection = rusqlite::Connection::open_with_flags(
        f.app.tmp.path().join("book_composition.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    connection
        .query_row(
            "SELECT composition_json FROM book_compositions WHERE book_key = ?1",
            [f.scope.storage_key()],
            |row| row.get(0),
        )
        .unwrap()
}

fn start_editing(app: &mut App, ctx: &egui::Context) {
    assert!(app.start_local_composition_editor(ctx));
    finish_prepare(app, ctx);
}

fn buffer_save_completion(app: &mut App) -> bool {
    assert_eq!(app.composition_editor_operations.tickets.len(), 1);
    let ticket = &mut app.composition_editor_operations.tickets[0];
    let completion = ticket
        .receiver
        .recv_timeout(WAIT)
        .expect("one save completion");
    let success = completion.status.is_ok();
    let (tx, rx) = mpsc::channel();
    tx.send(completion).unwrap();
    ticket.receiver = rx;
    success
}

fn stored(f: &Fixture) -> Option<StoredComposition> {
    f.service
        .prepare(f.scope.clone(), f.domain.clone())
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap()
}

fn fail_updates(f: &Fixture) {
    let lease = crate::book_composition::maintenance_at(f.app.tmp.path()).unwrap();
    let connection =
        rusqlite::Connection::open(f.app.tmp.path().join("book_composition.db")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_editor_save BEFORE UPDATE ON book_compositions BEGIN SELECT RAISE(ABORT,'injected editor save failure'); END").unwrap();
    drop(connection);
    drop(lease);
}

fn shift_row(f: &Fixture) -> crate::spread_shift::ShiftReadReply {
    let row = f
        .shift
        .read_after(f.scope.clone(), f.shift.watermarks().admitted)
        .recv_timeout(WAIT)
        .unwrap();
    assert!(row.status.is_ok());
    row
}

fn remote(app: &mut App) -> crate::remote_ipc::session::SessionHandle {
    let handle = crate::remote_ipc::session::SessionHandle::new();
    app.set_remote_session_handle(handle.clone());
    handle
}

fn acquire(handle: &crate::remote_ipc::session::SessionHandle) {
    assert_eq!(
        handle
            .acquire(mimageviewer_ipc::SessionAcquireRequest {
                client_id: "editor-phone".into(),
                peer: mimageviewer_ipc::SessionPeerInfo {
                    connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                    device_name: None
                }
            })
            .status,
        mimageviewer_ipc::SessionStatus::Active
    );
}

#[test]
fn c7_editor_create_edit_cancel_save_and_db_failure_never_write_shift() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    let before_watermarks = f.shift.watermarks();
    let before_row = shift_row(&f);
    // Opening an editor reads the canonical nav projection. Warm that derived cache so the
    // complete owner comparison below measures operation state rather than lazy materialization.
    let _ = f.app.get_still_image_indices();
    let before_live = f.app.spread_shift.clone();
    start_editing(&mut f.app, &ctx);
    f.app.apply_editor_ui_action(
        &ctx,
        ui::EditorUiAction::SetHidden {
            page: 1,
            hidden: true,
        },
    );
    f.app
        .close_local_composition_editor(CompositionEditorCloseReason::Cancel);
    assert!(stored(&f).is_none());
    assert_eq!(f.app.spread_shift, before_live);
    assert_eq!(f.shift.watermarks(), before_watermarks);
    assert_eq!(shift_row(&f), before_row);

    start_editing(&mut f.app, &ctx);
    f.app.apply_editor_ui_action(
        &ctx,
        ui::EditorUiAction::SetHidden {
            page: 1,
            hidden: true,
        },
    );
    finish_editor_results(&mut f.app);
    f.app.save_local_composition_editor(&ctx, false);
    assert!(buffer_save_completion(&mut f.app));
    f.app.poll_composition_editor_operations(&ctx);
    assert!(!f.app.composition_editor_writes_pending());
    let saved = stored(&f).expect("composition saved");
    assert!(
        saved
            .composition
            .hidden_pages
            .contains(&CompositionPageRef::ArchiveEntry {
                entry_name: "b.png".into()
            })
    );
    assert_eq!(f.shift.watermarks(), before_watermarks);
    assert_eq!(shift_row(&f), before_row);

    f.app
        .close_local_composition_editor(CompositionEditorCloseReason::Cancel);
    install_archive(&mut f.app, &f.tree);
    fail_updates(&f);
    start_editing(&mut f.app, &ctx);
    f.app.apply_editor_ui_action(
        &ctx,
        ui::EditorUiAction::SetHidden {
            page: 2,
            hidden: true,
        },
    );
    let CompositionEditorState::Editing(session) = &f.app.composition_editor else {
        panic!("Editing");
    };
    let draft = session.prepared.as_ref().unwrap().draft.record().clone();
    finish_editor_results(&mut f.app);
    f.app.save_local_composition_editor(&ctx, false);
    assert!(!buffer_save_completion(&mut f.app));
    f.app.poll_composition_editor_operations(&ctx);
    let CompositionEditorState::Editing(session) = &f.app.composition_editor else {
        panic!("failed save retains draft");
    };
    assert!(session.error.is_some());
    assert_eq!(session.prepared.as_ref().unwrap().draft.record(), &draft);
    assert_eq!(stored(&f).unwrap().composition, saved.composition);
    assert_eq!(f.shift.watermarks(), before_watermarks);
    assert_eq!(shift_row(&f), before_row);
}

#[test]
fn remote_acquire_closes_main_only_editor_and_prevents_inverse_start() {
    use crate::remote_ipc::session::RemoteControlPhase;
    for acquire_first in [false, true] {
        let mut f = fixture();
        let ctx = egui::Context::default();
        let handle = remote(&mut f.app);
        if acquire_first {
            acquire(&handle);
            assert_eq!(handle.snapshot().phase, RemoteControlPhase::AcquiringRemote);
            assert!(!f.app.start_local_composition_editor(&ctx));
            assert!(!f.app.composition_editor_modal_active());
        } else {
            start_editing(&mut f.app, &ctx);
            acquire(&handle);
        }
        assert!(f.app.fullscreen_idx.is_none());
        f.app.poll_remote_session(&ctx);
        assert!(matches!(
            f.app.composition_editor,
            CompositionEditorState::Closed
        ));
        assert!(!f.app.composition_editor_modal_active());
        assert_eq!(handle.snapshot().phase, RemoteControlPhase::RemoteActive);
        assert!(!f.app.start_local_composition_editor(&ctx));
        assert!(matches!(
            f.app.composition_editor,
            CompositionEditorState::Closed
        ));
    }
}

#[test]
fn source_changed_save_retains_draft_and_record_without_writing_shift() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    start_editing(&mut f.app, &ctx);
    let CompositionEditorState::Editing(session) = &f.app.composition_editor else {
        panic!("Editing");
    };
    let original = session.prepared.as_ref().unwrap().draft.record().clone();
    assert!(
        f.service
            .save(f.scope.clone(), f.domain.clone(), original.clone())
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .is_ok()
    );
    let before_record = stored(&f).unwrap();
    f.app.apply_editor_ui_action(
        &ctx,
        ui::EditorUiAction::SetHidden {
            page: 1,
            hidden: true,
        },
    );
    let CompositionEditorState::Editing(session) = &f.app.composition_editor else {
        panic!("Editing");
    };
    let draft = session.prepared.as_ref().unwrap().draft.record().clone();
    let before_row = shift_row(&f);
    let before_shift = f.shift.watermarks();
    let before_token = f.service.snapshot().token_for(&f.scope);
    // A changed length is deterministic even on filesystems with coarse timestamp resolution.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&f.tree.zip_path)
        .unwrap()
        .write_all(b"source changed after editor preparation")
        .unwrap();
    finish_editor_results(&mut f.app);
    f.app.save_local_composition_editor(&ctx, false);
    assert!(!buffer_save_completion(&mut f.app));
    f.app.poll_composition_editor_operations(&ctx);
    let CompositionEditorState::Editing(session) = &f.app.composition_editor else {
        panic!("source failure retains Editing");
    };
    assert_eq!(
        session.error.as_deref(),
        Some("ページが変更されています。読み直してください。")
    );
    assert_eq!(session.prepared.as_ref().unwrap().draft.record(), &draft);
    assert!(f.app.composition_editor_modal_active());
    assert!(!f.app.composition_editor_writes_pending());
    assert_eq!(stored(&f), Some(before_record));
    assert_eq!(f.service.snapshot().token_for(&f.scope), before_token);
    assert_eq!(f.shift.watermarks(), before_shift);
    assert_eq!(shift_row(&f), before_row);
}

#[test]
fn corrupt_saved_record_blocks_save_remove_and_ignore_is_this_open_only() {
    use egui_kittest::kittest::Queryable;
    let mut f = fixture();
    let ctx = egui::Context::default();
    start_editing(&mut f.app, &ctx);
    let CompositionEditorState::Editing(session) = &f.app.composition_editor else {
        panic!("Editing");
    };
    let record = session.prepared.as_ref().unwrap().draft.record().clone();
    assert!(
        f.service
            .save(f.scope.clone(), f.domain.clone(), record)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .is_ok()
    );
    f.app
        .close_local_composition_editor(CompositionEditorCloseReason::Cancel);
    let lease = crate::book_composition::maintenance_at(f.app.tmp.path()).unwrap();
    let connection =
        rusqlite::Connection::open(f.app.tmp.path().join("book_composition.db")).unwrap();
    connection
        .execute(
            "UPDATE book_compositions SET composition_json = ?1 WHERE book_key = ?2",
            rusqlite::params!["malformed saved record", f.scope.storage_key()],
        )
        .unwrap();
    drop(connection);
    drop(lease);
    assert!(
        f.service
            .prepare(f.scope.clone(), f.domain.clone())
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .is_err()
    );
    let before_token = f.service.snapshot().token_for(&f.scope);
    let before_shift = f.shift.watermarks();
    let before_row = shift_row(&f);
    #[cfg(windows)]
    let sibling = {
        let tree = Arc::clone(&f.tree);
        let scope = f.scope.clone();
        f.app.build_window_context_for_test(703, |app| {
            install_archive(app, &tree);
            app.book_composition = ViewerBookComposition::Blocked {
                scope,
                token: before_token,
                error: "sibling remains blocked".into(),
            };
        })
    };
    assert!(f.app.start_local_composition_editor(&ctx));
    deliver_prepare(&mut f.app, &ctx);
    let CompositionEditorState::Editing(session) = &f.app.composition_editor else {
        panic!("Blocked editor presentation");
    };
    assert!(session.prepared.is_err());
    assert_eq!(session.view.phase, ui::EditorPhase::Blocked);
    assert!(!session.view.can_save);
    assert!(!session.view.can_remove);
    for remove in [false, true] {
        f.app.save_local_composition_editor(&ctx, remove);
        assert!(!f.app.composition_editor_writes_pending());
        assert!(f.app.composition_editor_modal_active());
    }
    let trim_path = f.app.tmp.path().join("view_trim.db");
    f.app.view_trim_db = Some(crate::view_trim_db::ViewTrimDb::open_at(&trim_path).unwrap());
    let page_key = f.app.page_path_key(1).unwrap();
    let trim =
        crate::view_trim::ViewTrimPageOverride::from_margins(crate::view_trim::ViewTrimMargins {
            left: 0.1,
            ..Default::default()
        });
    f.app.view_trim_page_overrides.insert(1, trim);
    f.app.view_trim_dirty_page_overrides.insert(1);
    f.app.view_trim_save_pending = true;
    let old_order: Vec<_> = f
        .app
        .items
        .iter()
        .map(|item| PageIdentity::for_item(item, None).unwrap())
        .collect();
    {
        let mut fonts_ready = false;
        let mut h = crate::test_ui::builder()
            .with_size(egui::vec2(1093.0, 614.0))
            .build_ui(|root_ui| {
                let ctx = &root_ui.ctx().clone();
                if !fonts_ready {
                    crate::ui_fonts::configure_fonts(ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                f.app.update_ime_state(ctx);
                f.app.show_local_composition_editor(ctx);
            });
        h.run();
        h.get_by_label("今回は構成を使わず開く").click();
        h.run();
    }
    assert!(!f.app.composition_editor_modal_active());
    assert!(matches!(&f.app.book_composition,
        ViewerBookComposition::Projecting { scope, origin, .. }
        if scope == &f.scope && matches!(origin.as_ref(), ProjectionOrigin::IgnoreThisOpen { .. })));
    finish_projection(&mut f.app, &ctx);
    assert_eq!(
        f.app
            .items
            .iter()
            .map(|item| PageIdentity::for_item(item, None).unwrap())
            .collect::<Vec<_>>(),
        old_order
    );
    assert_eq!(
        crate::view_trim_db::ViewTrimDb::open_at(&trim_path)
            .unwrap()
            .get_page_override(&page_key),
        Some(trim)
    );
    assert!(!f.app.view_trim_save_pending);
    assert!(f.app.view_trim_dirty_page_overrides.is_empty());
    assert!(
        matches!(&f.app.book_composition, ViewerBookComposition::Ignored { scope, generation, .. }
        if scope == &f.scope && *generation == f.app.items_generation)
    );
    assert!(matches!(
        f.app.book_composition.clone(),
        ViewerBookComposition::Automatic
    ));
    f.app.poll_book_composition_read(&ctx);
    assert!(matches!(
        f.app.book_composition,
        ViewerBookComposition::Ignored { .. }
    ));
    #[cfg(windows)]
    {
        let selected_scope = f.scope.clone();
        f.app.with_viewer_context(sibling, |app| {
        assert!(matches!(&app.book_composition, ViewerBookComposition::Blocked { scope, error, .. }
            if scope == &selected_scope && error == "sibling remains blocked"));
    }).unwrap();
    }
    assert_eq!(record_json(&f), "malformed saved record");
    assert_eq!(f.service.snapshot().token_for(&f.scope), before_token);
    assert_eq!(f.shift.watermarks(), before_shift);
    assert_eq!(shift_row(&f), before_row);
    // A fresh open has no copy of the per-viewer ignore decision.
    install_archive(&mut f.app, &f.tree);
    assert!(f.app.start_local_composition_editor(&ctx));
    deliver_prepare(&mut f.app, &ctx);
    assert!(
        matches!(&f.app.composition_editor, CompositionEditorState::Editing(session)
        if session.prepared.is_err() && session.view.phase == ui::EditorPhase::Blocked)
    );
}

#[test]
fn preparing_reply_after_begin_acquire_cannot_revive_editor_before_remote_poll() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    let handle = remote(&mut f.app);
    let (entered, release) = f.service.hold_worker_for_test();
    entered.recv_timeout(WAIT).unwrap();
    assert!(f.app.start_local_composition_editor(&ctx));
    let CompositionEditorState::Preparing { input, receiver } =
        std::mem::take(&mut f.app.composition_editor)
    else {
        panic!("Preparing");
    };
    acquire(&handle);
    release.send(()).unwrap();
    let late = receiver.recv_timeout(WAIT).unwrap();
    let (tx, rx) = mpsc::channel();
    tx.send(late).unwrap();
    f.app.composition_editor = CompositionEditorState::Preparing {
        input,
        receiver: rx,
    };
    f.app.poll_composition_editor_operations(&ctx);
    assert!(matches!(
        f.app.composition_editor,
        CompositionEditorState::Closed
    ));
    assert!(!f.app.composition_editor_modal_active());
    f.app.poll_remote_session(&ctx);
    assert!(matches!(
        f.app.composition_editor,
        CompositionEditorState::Closed
    ));
}

#[test]
fn cancelled_preparing_request_drops_late_reply_without_a_new_owner() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    let (entered, release) = f.service.hold_worker_for_test();
    entered.recv_timeout(WAIT).unwrap();
    assert!(f.app.start_local_composition_editor(&ctx));
    let CompositionEditorState::Preparing { input, receiver } =
        std::mem::take(&mut f.app.composition_editor)
    else {
        panic!("Preparing");
    };
    let (late_delivery, ghost_receiver) = mpsc::channel();
    f.app.composition_editor = CompositionEditorState::Preparing {
        input,
        receiver: ghost_receiver,
    };
    let owner_context = f.app.projected_viewer_context_id();
    f.app.close_composition_editor_for_context(owner_context);
    assert!(f.app.composition_editor_acquire_ready());
    release.send(()).unwrap();
    let late_reply = receiver.recv_timeout(WAIT).unwrap();
    assert!(
        late_delivery.send(late_reply).is_err(),
        "retired receiver is gone"
    );
    f.app.poll_composition_editor_operations(&ctx);
    assert!(matches!(
        f.app.composition_editor,
        CompositionEditorState::Closed
    ));
    assert!(!f.app.composition_editor_modal_active());
    assert!(stored(&f).is_none());
}

#[test]
fn remote_save_barrier_waits_for_harvested_success_and_failure_once() {
    use crate::remote_ipc::session::RemoteControlPhase;
    for failure in [false, true] {
        let mut f = fixture();
        let ctx = egui::Context::default();
        start_editing(&mut f.app, &ctx);
        if failure {
            let CompositionEditorState::Editing(session) = &f.app.composition_editor else {
                panic!("Editing");
            };
            let record = session.prepared.as_ref().unwrap().draft.record().clone();
            assert!(
                f.service
                    .save(f.scope.clone(), f.domain.clone(), record)
                    .recv_timeout(WAIT)
                    .unwrap()
                    .status
                    .is_ok()
            );
            fail_updates(&f);
        }
        let before = stored(&f);
        f.app.apply_editor_ui_action(
            &ctx,
            ui::EditorUiAction::SetHidden {
                page: 1,
                hidden: true,
            },
        );
        let handle = remote(&mut f.app);
        let (entered, release) = f.service.hold_worker_for_test();
        entered.recv_timeout(WAIT).unwrap();
        finish_editor_results(&mut f.app);
        f.app.save_local_composition_editor(&ctx, false);
        assert!(matches!(
            f.app.composition_editor,
            CompositionEditorState::Saving(_)
        ));
        acquire(&handle);
        f.app.poll_remote_session(&ctx);
        assert!(matches!(
            f.app.composition_editor,
            CompositionEditorState::Closed
        ));
        assert_eq!(handle.snapshot().phase, RemoteControlPhase::AcquiringRemote);
        assert!(f.app.composition_editor_writes_pending());
        release.send(()).unwrap();
        assert_eq!(buffer_save_completion(&mut f.app), !failure);
        // Worker completion alone cannot let the remote first read overtake the receipt.
        f.app.poll_remote_session(&ctx);
        assert_eq!(handle.snapshot().phase, RemoteControlPhase::AcquiringRemote);
        f.app.poll_composition_editor_operations(&ctx);
        assert!(f.app.composition_editor_acquire_ready());
        f.app.poll_remote_session(&ctx);
        assert_eq!(handle.snapshot().phase, RemoteControlPhase::RemoteActive);
        assert!(matches!(
            f.app.composition_editor,
            CompositionEditorState::Closed
        ));
        let token = f.service.snapshot().token_for(&f.scope);
        f.app.poll_composition_editor_operations(&ctx);
        assert!(f.app.composition_editor_operations.tickets.is_empty());
        assert_eq!(f.service.snapshot().token_for(&f.scope), token);
        if failure {
            assert_eq!(stored(&f).unwrap().composition, before.unwrap().composition);
        } else {
            assert!(stored(&f).unwrap().composition.hidden_pages.contains(
                &CompositionPageRef::ArchiveEntry {
                    entry_name: "b.png".into()
                }
            ));
        }
    }
}

#[cfg(windows)]
#[test]
fn temporary_sibling_mount_keeps_global_modal_owner_and_rejects_preferences() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    let sibling = f.app.build_window_context_for_test(701, |_| {});
    start_editing(&mut f.app, &ctx);
    let owner = f.app.composition_editor_operations.owner.clone().unwrap();
    f.app
        .with_window_viewer_context(701, |app| {
            assert_eq!(app.projected_viewer_context_id(), sibling);
            assert!(matches!(
                app.composition_editor,
                CompositionEditorState::Closed
            ));
            assert_eq!(
                app.composition_editor_operations.owner.as_ref(),
                Some(&owner)
            );
            assert!(app.composition_editor_modal_active());
            assert!(!app.start_local_composition_editor(&ctx));
            app.open_preferences_request(
                crate::ui_dialogs::preferences::PreferencesOpenRequest::FILE_TYPE_VISIBILITY,
            );
            assert!(!app.show_preferences);
            app.close_local_composition_editor(CompositionEditorCloseReason::OwnerClosed);
            assert!(matches!(
                app.composition_editor,
                CompositionEditorState::Closed
            ));
        })
        .unwrap();
    assert!(matches!(
        f.app.composition_editor,
        CompositionEditorState::Closed
    ));
    assert!(!f.app.composition_editor_modal_active());
    f.app.open_preferences_request(
        crate::ui_dialogs::preferences::PreferencesOpenRequest::FILE_TYPE_VISIBILITY,
    );
    assert!(f.app.show_preferences);
    f.app.show_preferences = false;
}

#[cfg(windows)]
#[test]
fn owner_context_retire_terminal_closes_editor_without_touching_main() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    let tree = Arc::clone(&f.tree);
    let owner_context = f
        .app
        .build_window_context_for_test(702, |app| install_archive(app, &tree));
    let main_items = f.app.items.clone();
    let main_generation = f.app.items_generation;
    f.app
        .with_viewer_context(owner_context, |app| start_editing(app, &ctx))
        .unwrap();
    assert!(matches!(
        f.app.composition_editor,
        CompositionEditorState::Closed
    ));
    assert!(f.app.composition_editor_modal_active());
    f.app
        .retire_context(owner_context, "editor_owner_test_retire", |_| ())
        .unwrap();
    assert!(!f.app.composition_editor_modal_active());
    assert!(f.app.composition_editor_acquire_ready());
    assert_eq!(f.app.items, main_items);
    assert_eq!(f.app.items_generation, main_generation);
}

#[test]
fn saved_receipt_records_resume_once_before_reload_and_failed_save_records_none() {
    for failure in [false, true] {
        let mut f = fixture();
        let ctx = egui::Context::default();
        f.app.book_resume_writer = crate::book_resume_db::BookResumeWriter::spawn_at(
            f.app.tmp.path().join("editor-resume.db"),
        );
        assert!(f.app.book_resume_writer.is_some());
        // A stable open has already accepted the automatic publication. Saving then follows
        // the same Refresh path as a real viewer, including its no-resume-write adoption.
        accept_automatic(&mut f.app, &f.service, &f.scope);
        f.app.fullscreen_idx = Some(1);
        f.app.reading_flow = crate::settings::ReadingFlow::Horizontal;
        f.app.fs_zoom = 1.37;
        f.app.fs_pan = egui::vec2(17.0, -6.0);
        let view_mode = f.app.spread_mode;
        let source = f.app.current_folder.clone();
        start_editing(&mut f.app, &ctx);
        if failure {
            let CompositionEditorState::Editing(session) = &f.app.composition_editor else {
                panic!("Editing");
            };
            let record = session.prepared.as_ref().unwrap().draft.record().clone();
            assert!(
                f.service
                    .save(f.scope.clone(), f.domain.clone(), record)
                    .recv_timeout(WAIT)
                    .unwrap()
                    .status
                    .is_ok()
            );
            fail_updates(&f);
        }
        f.app.apply_editor_ui_action(
            &ctx,
            ui::EditorUiAction::SetHidden {
                page: 1,
                hidden: true,
            },
        );
        let before = f
            .app
            .book_resume_writer
            .as_ref()
            .unwrap()
            .watermarks()
            .admitted;
        finish_editor_results(&mut f.app);
        f.app.save_local_composition_editor(&ctx, false);
        assert_eq!(buffer_save_completion(&mut f.app), !failure);
        assert_eq!(
            f.app
                .book_resume_writer
                .as_ref()
                .unwrap()
                .watermarks()
                .admitted,
            before,
            "worker completion alone cannot publish resume"
        );
        f.app.poll_composition_editor_operations(&ctx);
        let writer = f.app.book_resume_writer.as_ref().unwrap();
        let cutoff = writer.watermarks().admitted;
        assert_eq!(cutoff, before + u64::from(!failure));
        let reply = writer
            .read_position_after(&crate::path_key::normalize(&f.scope.logical_root), cutoff)
            .recv_timeout(WAIT)
            .unwrap();
        assert!(reply.status.is_ok());
        if failure {
            assert!(reply.position.is_none());
            assert!(matches!(
                f.app.composition_editor,
                CompositionEditorState::Editing(_)
            ));
        } else {
            let position = reply
                .position
                .expect("one receipt publishes successor position");
            assert_eq!(
                position.identity,
                Some(
                    CompositionPageRef::ArchiveEntry {
                        entry_name: "c.png".into(),
                    }
                    .resume_identity(&f.scope.logical_root)
                )
            );
        }
        assert_eq!(f.app.reading_flow, crate::settings::ReadingFlow::Horizontal);
        assert_eq!(f.app.fs_zoom, 1.37);
        assert_eq!(f.app.fs_pan, egui::vec2(17.0, -6.0));
        // Neither repeated harvesting nor the subsequent projection is another receipt writer.
        f.app.poll_composition_editor_operations(&ctx);
        if !failure {
            finish_read_projection(&mut f.app, &ctx);
            f.app.poll_composition_editor_operations(&ctx);
            assert!(!f.app.composition_editor_modal_active());
            assert_eq!(
                fullscreen_identity(&f.app),
                CompositionPageRef::ArchiveEntry {
                    entry_name: "c.png".into(),
                }
                .resume_identity(&f.scope.logical_root)
            );
            if let Some(index) = f.app.fullscreen_idx {
                f.app.record_book_resume(index);
            }
        }
        assert_eq!(f.app.reading_flow, crate::settings::ReadingFlow::Horizontal);
        assert_eq!(f.app.spread_mode, view_mode);
        assert_eq!(f.app.fs_zoom, 1.37);
        assert_eq!(f.app.fs_pan, egui::vec2(17.0, -6.0));
        assert_eq!(f.app.current_folder, source);
        assert_eq!(
            f.app
                .book_resume_writer
                .as_ref()
                .unwrap()
                .watermarks()
                .admitted,
            cutoff
        );
        assert!(!f.app.composition_editor_writes_pending());
    }
}

#[cfg(windows)]
#[test]
fn shared_book_owner_save_reanchors_a20_b80_in_both_reload_completion_orders() {
    for sibling_first in [false, true] {
        let pages = (0..100)
            .map(|page| format!("p{page:03}.png"))
            .collect::<Vec<_>>();
        let mut f = fixture_with_pages(&pages);
        let ctx = egui::Context::default();
        f.app.book_resume_writer = crate::book_resume_db::BookResumeWriter::spawn_at(
            f.app.tmp.path().join("shared-book-editor-resume.db"),
        );
        assert!(f.app.book_resume_writer.is_some());
        accept_automatic(&mut f.app, &f.service, &f.scope);
        f.app.fullscreen_idx = Some(20);
        let tree = Arc::clone(&f.tree);
        let service = f.service.clone();
        let scope = f.scope.clone();
        let sibling = f.app.build_window_context_for_test(706, |app| {
            install_archive(app, &tree);
            accept_automatic(app, &service, &scope);
            app.fullscreen_idx = Some(80);
        });
        let before_shift = f.shift.watermarks();
        let before_row = shift_row(&f);
        start_editing(&mut f.app, &ctx);
        for page in [20, 80] {
            f.app
                .apply_editor_ui_action(&ctx, ui::EditorUiAction::SetHidden { page, hidden: true });
        }
        // Hold the serial store after preparation so both published viewers remain on the
        // old rows until the sole editor-save receipt has been harvested.
        let (entered, release) = f.service.hold_worker_for_test();
        entered.recv_timeout(WAIT).unwrap();
        finish_editor_results(&mut f.app);
        f.app.save_local_composition_editor(&ctx, false);
        f.app.poll_book_composition_service_for_test(&ctx);
        assert_eq!(f.app.items.len(), 100);
        assert!(matches!(
            f.app.book_composition,
            ViewerBookComposition::Accepted { .. }
        ));
        f.app
            .with_viewer_context(sibling, |app| {
                assert_eq!(app.items.len(), 100);
                assert!(matches!(
                    app.book_composition,
                    ViewerBookComposition::Accepted { .. }
                ));
            })
            .unwrap();
        release.send(()).unwrap();
        assert!(buffer_save_completion(&mut f.app));
        assert_eq!(
            f.app
                .book_resume_writer
                .as_ref()
                .unwrap()
                .watermarks()
                .admitted,
            0
        );
        f.app.poll_composition_editor_operations(&ctx);
        assert_eq!(
            f.app
                .book_resume_writer
                .as_ref()
                .unwrap()
                .watermarks()
                .admitted,
            1
        );

        // Start both real projection workers, retaining their receivers so consumption order
        // is explicit and independent of which worker finishes first.
        advance_read_to_projection(&mut f.app, &ctx);
        f.app
            .with_viewer_context(sibling, |app| {
                app.poll_book_composition_read(&ctx);
                advance_read_to_projection(app, &ctx);
            })
            .unwrap();
        let sibling_completion = |app: &mut App| {
            app.with_viewer_context(sibling, |viewer| {
                finish_projection(viewer, &ctx);
                assert_eq!(
                    fullscreen_identity(viewer),
                    CompositionPageRef::ArchiveEntry {
                        entry_name: "p081.png".into(),
                    }
                    .resume_identity(&scope.logical_root)
                );
                let index = viewer.fullscreen_idx.unwrap();
                viewer.record_book_resume(index);
            })
            .unwrap();
        };
        if sibling_first {
            sibling_completion(&mut f.app);
        }
        finish_projection(&mut f.app, &ctx);
        f.app.poll_composition_editor_operations(&ctx);
        assert!(!f.app.composition_editor_modal_active());
        assert_eq!(
            fullscreen_identity(&f.app),
            CompositionPageRef::ArchiveEntry {
                entry_name: "p021.png".into(),
            }
            .resume_identity(&f.scope.logical_root)
        );
        let index = f.app.fullscreen_idx.unwrap();
        f.app.record_book_resume(index);
        if !sibling_first {
            sibling_completion(&mut f.app);
        }
        assert_eq!(f.app.items.len(), 98);
        f.app
            .with_viewer_context(sibling, |app| {
                assert_eq!(app.items.len(), 98);
                assert_eq!(
                    fullscreen_identity(app),
                    CompositionPageRef::ArchiveEntry {
                        entry_name: "p081.png".into(),
                    }
                    .resume_identity(&scope.logical_root)
                );
            })
            .unwrap();
        let writer = f.app.book_resume_writer.as_ref().unwrap();
        assert_eq!(
            writer.watermarks().admitted,
            1,
            "only A's save receipt may publish the shared reading position"
        );
        let reply = writer
            .read_position_after(&crate::path_key::normalize(&scope.logical_root), 1)
            .recv_timeout(WAIT)
            .unwrap();
        assert!(reply.status.is_ok());
        let position = reply.position.expect("A's receipt publishes a position");
        assert_eq!(position.page, 21);
        assert_eq!(
            position.identity,
            Some(
                CompositionPageRef::ArchiveEntry {
                    entry_name: "p021.png".into(),
                }
                .resume_identity(&scope.logical_root)
            )
        );
        f.app.poll_composition_editor_operations(&ctx);
        assert!(f.app.composition_editor_acquire_ready());
        assert_eq!(
            f.app
                .book_resume_writer
                .as_ref()
                .unwrap()
                .watermarks()
                .admitted,
            1
        );
        assert_eq!(f.shift.watermarks(), before_shift);
        assert_eq!(shift_row(&f), before_row);
    }
}

#[cfg(windows)]
#[test]
fn retired_owner_save_receipt_records_once_without_reviving_owner_or_touching_main() {
    for failure in [false, true] {
        let mut f = fixture();
        let ctx = egui::Context::default();
        f.app.book_resume_writer = crate::book_resume_db::BookResumeWriter::spawn_at(
            f.app.tmp.path().join("retired-editor-resume.db"),
        );
        assert!(f.app.book_resume_writer.is_some());
        let tree = Arc::clone(&f.tree);
        let owner = f.app.build_window_context_for_test(704, |app| {
            install_archive(app, &tree);
            app.fullscreen_idx = Some(1);
        });
        f.app
            .with_viewer_context(owner, |app| start_editing(app, &ctx))
            .unwrap();
        if failure {
            let record = f
                .app
                .with_viewer_context(owner, |app| {
                    let CompositionEditorState::Editing(session) = &app.composition_editor else {
                        panic!("Editing");
                    };
                    session.prepared.as_ref().unwrap().draft.record().clone()
                })
                .unwrap();
            assert!(
                f.service
                    .save(f.scope.clone(), f.domain.clone(), record)
                    .recv_timeout(WAIT)
                    .unwrap()
                    .status
                    .is_ok()
            );
            fail_updates(&f);
        }
        let before = stored(&f);
        let main_items = f.app.items.clone();
        let main_generation = f.app.items_generation;
        let (entered, release) = f.service.hold_worker_for_test();
        entered.recv_timeout(WAIT).unwrap();
        f.app
            .with_viewer_context(owner, |app| {
                app.apply_editor_ui_action(
                    &ctx,
                    ui::EditorUiAction::SetHidden {
                        page: 1,
                        hidden: true,
                    },
                );
                finish_editor_results(app);
                app.save_local_composition_editor(&ctx, false);
            })
            .unwrap();
        assert!(f.app.composition_editor_writes_pending());
        f.app
            .retire_context(owner, "retire_sent_editor_save", |_| ())
            .unwrap();
        assert!(!f.app.composition_editor_modal_active());
        assert!(!f.app.composition_editor_acquire_ready());
        release.send(()).unwrap();
        assert_eq!(buffer_save_completion(&mut f.app), !failure);
        f.app.poll_composition_editor_operations(&ctx);
        assert!(f.app.composition_editor_acquire_ready());
        let writer = f.app.book_resume_writer.as_ref().unwrap();
        let cutoff = writer.watermarks().admitted;
        assert_eq!(cutoff, u64::from(!failure));
        let reply = writer
            .read_position_after(&crate::path_key::normalize(&f.scope.logical_root), cutoff)
            .recv_timeout(WAIT)
            .unwrap();
        assert!(reply.status.is_ok());
        assert_eq!(reply.position.is_some(), !failure);
        f.app.poll_composition_editor_operations(&ctx);
        assert_eq!(
            f.app
                .book_resume_writer
                .as_ref()
                .unwrap()
                .watermarks()
                .admitted,
            cutoff
        );
        assert!(matches!(
            f.app.composition_editor,
            CompositionEditorState::Closed
        ));
        if failure {
            assert!(f.app.last_book_resume.is_none());
        } else {
            // The dedup mirror belongs to the shared writer, not the mounted viewer.
            let (path, page, meter, identity) = f.app.last_book_resume.clone().unwrap();
            let position = reply.position.as_ref().unwrap();
            assert_eq!(
                crate::path_key::normalize(&path),
                crate::path_key::normalize(&f.scope.logical_root)
            );
            assert_eq!(page, position.page);
            assert_eq!(identity, position.identity);
            assert!(meter.is_some());
            f.app
                .persist_book_resume_with_identity(path, page, meter, identity);
            assert_eq!(
                f.app
                    .book_resume_writer
                    .as_ref()
                    .unwrap()
                    .watermarks()
                    .admitted,
                cutoff
            );
        }
        assert_eq!(f.app.items, main_items);
        assert_eq!(f.app.items_generation, main_generation);
        if failure {
            assert_eq!(stored(&f), before);
        }
    }
}

#[cfg(windows)]
#[test]
fn modal_owner_rejects_external_activation_and_file_drop_during_temporary_mount() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    let destination = f.app.tmp.path().join("drop-destination");
    std::fs::create_dir(&destination).unwrap();
    let source = f.app.tmp.path().join("drop-source.txt");
    std::fs::write(&source, "disposable source").unwrap();
    let sibling = f.app.build_window_context_for_test(705, |app| {
        app.current_folder = Some(destination.clone());
    });
    start_editing(&mut f.app, &ctx);
    let owner = f.app.composition_editor_operations.owner.clone();
    f.app
        .activation_open_path_tx
        .send(crate::single_instance::ActivationOpenPath::received(
            source.clone(),
            None,
        ))
        .unwrap();
    f.app
        .with_viewer_context(sibling, |app| {
            assert!(app.composition_editor_modal_active());
            app.poll_activation_open_paths(&ctx);
            assert!(app.startup_open_path_resolve_pending.is_none());
            assert!(matches!(
                app.activation_open_path_rx.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
            app.handle_external_file_drop(vec![source.clone()]);
            assert!(app.drop_copy_pending.is_empty());
            assert_eq!(
                app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
                Some("本の構成を編集中のため、ファイルを操作できません")
            );
            assert_eq!(app.current_folder.as_deref(), Some(destination.as_path()));
        })
        .unwrap();
    assert_eq!(f.app.composition_editor_operations.owner, owner);
    assert!(!destination.join("drop-source.txt").exists());
    f.app
        .close_local_composition_editor(CompositionEditorCloseReason::Cancel);
    f.app.poll_activation_open_paths(&ctx);
    assert!(
        f.app.startup_open_path_resolve_pending.is_none(),
        "rejected activation was consumed"
    );
}

#[test]
fn undo_and_redo_accept_immediate_draft_changes_while_result_is_pending() {
    let mut f = fixture();
    let ctx = egui::Context::default();
    start_editing(&mut f.app, &ctx);
    let before = draft_record(&f);
    let CompositionEditorState::Editing(session) = &mut f.app.composition_editor else {
        panic!("Editing");
    };
    let (_sender, receiver) = std::sync::mpsc::channel();
    session.result_worker.as_mut().unwrap().receiver = receiver;
    f.app.apply_editor_ui_action(
        &ctx,
        ui::EditorUiAction::SetOverride {
            page: 1,
            directive: Some(PageOverride::Alone),
        },
    );
    let edited = draft_record(&f);
    assert_ne!(edited, before);
    f.app.apply_editor_ui_action(&ctx, ui::EditorUiAction::Undo);
    assert_eq!(draft_record(&f), before);
    f.app.apply_editor_ui_action(&ctx, ui::EditorUiAction::Redo);
    assert_eq!(draft_record(&f), edited);
    let CompositionEditorState::Editing(session) = &f.app.composition_editor else {
        panic!("Editing");
    };
    assert!(session.view.results_updating);
    assert!(session.view.can_undo);
    assert!(!session.view.can_save);
}
