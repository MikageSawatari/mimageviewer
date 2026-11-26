use super::*;
use crate::export_dialog::{
    ExportEnd, ExportEvent, ExportFailure, ExportOrigin, ExportPending, ExportSuccess,
};
use crate::test_ui::scroll_layout::{HeightExpectation, ScrollLayout};
use egui_kittest::{Harness, kittest::Queryable};
use std::sync::{Arc, atomic::AtomicBool, mpsc};

fn connection(
    state: &mut CombineDialogState,
) -> (
    mpsc::Receiver<Command>,
    mpsc::Sender<Event>,
    Arc<AtomicBool>,
) {
    let (tx, commands) = mpsc::channel();
    let (events, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    state.worker = Some(worker::Connection {
        tx,
        rx,
        cancel: cancel.clone(),
    });
    (commands, events, cancel)
}

fn pending() -> (ExportPending, mpsc::Sender<ExportEvent>) {
    let (tx, rx) = mpsc::channel();
    (
        ExportPending {
            origin: ExportOrigin::Combine,
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            total: 3,
            done: 0,
            last_message: String::new(),
            successes: vec![],
            errors: vec![],
            skipped: vec![],
            thumbnail_notices: vec![],
            end: None,
            cancel_requested: false,
        },
        tx,
    )
}

#[test]
fn image_combine_entry_limits_and_modal_owner_reentry() {
    for count in [0, 1, 100, 101] {
        assert_eq!(
            entry_disabled_reason(count, false, false).is_none(),
            (1..=100).contains(&count)
        );
        assert!(
            entry_disabled_reason(count, true, false)
                .unwrap()
                .contains("結合の操作中")
        );
        assert!(
            entry_disabled_reason(count, false, true)
                .unwrap()
                .contains("エクスポート中")
        );
    }
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    let mut state = fixture_state(&ctx, false);
    let (_commands, _events, cancel) = connection(&mut state);
    app.combine_session = Some(CombineSession::Dialog(Box::new(state)));
    assert_eq!(app.modal_dialog_block_reason(), Some("image_combine"));
    assert!(app.document_open_modal_admission_blocked());
    app.open_image_combine_dialog(&ctx);
    assert!(matches!(
        app.combine_session,
        Some(CombineSession::Dialog(_))
    ));
    app.combine_session = None;
    assert!(cancel.load(Ordering::Relaxed));
    assert!(!app.document_open_modal_admission_blocked());
    app.combine_session = Some(CombineSession::Writing { request: 1 });
    assert!(!app.document_open_modal_admission_blocked());
    assert!(
        app.grid_combine_disabled_reason()
            .unwrap()
            .contains("結合の操作中")
    );
}

#[test]
fn image_combine_entry_selection_is_checked_first_and_counts_eligible_targets() {
    use crate::grid_item::GridItem;
    let mut app = crate::app::setup_app_for_test();
    app.items = (0..101)
        .map(|id| GridItem::Image(PathBuf::from(format!("C:/images/{id}.png"))))
        .collect();
    app.visible_indices = (0..101).collect();
    app.selected = Some(0);
    assert!(app.grid_combine_disabled_reason().is_none());
    app.checked = (0..100).collect();
    assert!(app.grid_combine_disabled_reason().is_none());
    app.checked.insert(100);
    assert!(
        app.grid_combine_disabled_reason()
            .unwrap()
            .contains("101 枚")
    );
    app.items[0] = GridItem::Video("C:/images/video.mp4".into());
    app.checked.clear();
    assert!(app.grid_combine_disabled_reason().is_some());
    app.checked.insert(1);
    assert!(app.grid_combine_disabled_reason().is_none());
}

#[test]
fn image_combine_escape_closes_and_late_worker_result_has_no_receiver() {
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    let mut app = crate::app::setup_app_for_test();
    let mut state = fixture_state(&ctx, false);
    let (_commands, events, cancel) = connection(&mut state);
    app.combine_session = Some(CombineSession::Dialog(Box::new(state)));
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
    assert!(
        events
            .send(Event::Failed {
                id: 0,
                message: "late".into()
            })
            .is_err()
    );
}

#[test]
fn image_combine_preparation_failure_is_retained_until_explicit_removal() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    let mut state = fixture_state(&ctx, false);
    let (commands, _events, _cancel) = connection(&mut state);
    let count = state.items.len();
    state.pending.push_back((
        0,
        ExportBatchTarget::Item {
            idx: usize::MAX,
            page_key: "missing".into(),
        },
    ));
    app.advance_image_combine_preparation(&mut state);
    assert_eq!(state.items.len(), count);
    assert!(matches!(state.items[0].status, ItemStatus::Failed(_)));
    assert!(!state.can_write());
    state.remove_failed();
    assert_eq!(state.items.len(), count - 1);
    assert!(matches!(commands.try_recv().unwrap(),Command::Remove{ids} if ids==vec![0]));
}

#[test]
fn image_combine_preview_generation_discards_stale_results_and_reorder_keeps_labels() {
    let ctx = egui::Context::default();
    let mut state = fixture_state(&ctx, true);
    let (_commands, events, _) = connection(&mut state);
    state.generation = 8;
    state.requested = Some(state.spec());
    let layout = state.validation().unwrap();
    for generation in [7, 8] {
        events
            .send(Event::Rendered {
                generation,
                rgba: image::RgbaImage::new(10, 12),
                layout: layout.clone(),
                not_drawn: 0,
            })
            .unwrap();
    }
    state.poll(&ctx);
    assert_eq!(state.preview.as_ref().unwrap().generation, 8);
    assert_eq!(state.preview.as_ref().unwrap().texture.size(), [10, 12]);
    state.move_item(2, Some(0));
    assert_eq!(state.items[0].id, 2);
    assert_eq!(state.items[0].spec.label_override.as_deref(), Some("商品3"));
    assert_eq!(
        state.preview.as_ref().unwrap().ids,
        [0, 1, 2, 3],
        "hit testing stays with the displayed image"
    );
    state.invalidate_and_request(&ctx);
    assert_eq!(state.generation, 9);
    assert!(!state.can_write());
}

#[test]
fn image_combine_poll_step_finish_refresh_selection_and_toast_exactly_once() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    let folder = app.tmp.path().join("current");
    std::fs::create_dir(&folder).unwrap();
    app.current_folder = Some(folder.clone());
    app.current_folder_last_mtime = Some(std::time::SystemTime::now());
    let path = folder.join("combined.png");
    app.combine_session = Some(CombineSession::Writing { request: 42 });
    let (p, tx) = pending();
    app.export_pending = Some(p);
    tx.send(ExportEvent::Step {
        label: "one".into(),
    })
    .unwrap();
    app.poll_export_pending(&ctx);
    let p = app.export_pending.as_ref().unwrap();
    assert_eq!(p.done, 1);
    assert!(p.successes.is_empty());
    assert!(p.errors.is_empty());
    tx.send(ExportEvent::Step {
        label: "two".into(),
    })
    .unwrap();
    tx.send(ExportEvent::Completed(ExportSuccess {
        label: "combined".into(),
        path: path.clone(),
    }))
    .unwrap();
    tx.send(ExportEvent::AllDone).unwrap();
    drop(tx);
    app.poll_export_pending(&ctx);
    assert!(app.export_pending.is_none());
    assert!(app.combine_session.is_none());
    assert!(
        matches!(&app.fs_feedback_toast_action,Some(crate::feedback::FeedbackAction::OpenInViewer(p)) if *p==path)
    );
    assert!(app.post_operation_selection.is_some());
    assert_eq!(app.folder_refresh_pending.as_ref().unwrap().folder, folder);
    app.fs_feedback_toast_action = None;
    app.folder_refresh_pending = None;
    app.poll_export_pending(&ctx);
    app.finish_combine_write();
    assert!(app.fs_feedback_toast_action.is_none());
    assert!(app.folder_refresh_pending.is_none());
}

#[test]
fn image_combine_finish_selection_depends_on_output_folder_not_favorite_eligibility() {
    for same_folder in [true, false] {
        let mut app = crate::app::setup_app_for_test();
        let books = app.tmp.path().join("books");
        let folder = books.join("current");
        app.settings.book_root = Some(books);
        app.current_folder = Some(folder.clone());
        app.current_folder_last_mtime = Some(std::time::SystemTime::now());
        assert!(app.current_favorite_target().is_none());
        let path = if same_folder {
            folder.join("combined.png")
        } else {
            app.tmp.path().join("other/combined.png")
        };
        let (mut p, _tx) = pending();
        p.end = Some(ExportEnd::Done);
        p.successes.push(ExportSuccess {
            label: "combined".into(),
            path,
        });
        app.export_pending = Some(p);
        app.combine_session = Some(CombineSession::Writing { request: 1 });
        app.finish_combine_write();
        assert_eq!(app.post_operation_selection.is_some(), same_folder);
    }
}

#[test]
fn image_combine_poll_disconnect_cancel_and_failed_completion_release_session() {
    for terminal in [None, Some(ExportEnd::Cancelled), Some(ExportEnd::Done)] {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        app.combine_session = Some(CombineSession::Writing { request: 1 });
        let (p, tx) = pending();
        app.export_pending = Some(p);
        if terminal == Some(ExportEnd::Done) {
            tx.send(ExportEvent::Failed(ExportFailure {
                label: "image".into(),
                message: "failed".into(),
            }))
            .unwrap();
            tx.send(ExportEvent::AllDone).unwrap();
        }
        if terminal == Some(ExportEnd::Cancelled) {
            tx.send(ExportEvent::Cancelled).unwrap();
        }
        drop(tx);
        app.poll_export_pending(&ctx);
        assert!(app.combine_session.is_none());
        if terminal == Some(ExportEnd::Done) {
            assert_eq!(app.export_pending.as_ref().unwrap().errors.len(), 1);
            app.poll_export_pending(&ctx);
            assert!(app.export_pending.is_some());
        } else {
            assert!(app.export_pending.is_none());
        }
        assert!(app.fs_feedback_toast_action.is_none());
    }
}

#[test]
fn image_combine_disconnect_after_completed_without_terminal_is_interruption() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    let (p, tx) = pending();
    app.export_pending = Some(p);
    app.combine_session = Some(CombineSession::Writing { request: 1 });
    tx.send(ExportEvent::Completed(ExportSuccess {
        label: "combined".into(),
        path: app.tmp.path().join("combined.png"),
    }))
    .unwrap();
    drop(tx);
    app.poll_export_pending(&ctx);
    assert!(app.combine_session.is_none());
    assert!(app.export_pending.is_none());
    assert!(app.fs_feedback_toast_action.is_none());
    assert!(app.fs_feedback_toast.as_ref().unwrap().0.contains("中断"));
    assert!(app.post_operation_selection.is_none());
}

#[test]
fn image_combine_feedback_click_queues_existing_explicit_path_request() {
    let mut app = crate::app::setup_app_for_test();
    let path = app.tmp.path().join("combined.png");
    app.activate_feedback_action(
        &crate::feedback::FeedbackAction::OpenInViewer(path.clone()),
        &egui::Context::default(),
    );
    assert_eq!(app.activation_open_path_rx.try_recv().unwrap().path, path);
    assert!(app.activation_open_path_rx.try_recv().is_err());
}

fn progress_harness(
    app: crate::app::AppTestEnvForTest,
) -> Harness<'static, crate::app::AppTestEnvForTest> {
    crate::test_ui::builder()
        .with_size(egui::vec2(1093.0, 614.0))
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .build_ui_state(
            |ui, app: &mut crate::app::AppTestEnvForTest| app.draw_export_progress_dialog(ui.ctx()),
            app,
        )
}

#[test]
fn image_combine_progress_cancel_keeps_owner_until_terminal() {
    let mut app = crate::app::setup_app_for_test();
    let (p, tx) = pending();
    let cancel = p.cancel.clone();
    app.export_pending = Some(p);
    app.combine_session = Some(CombineSession::Writing { request: 1 });
    let mut h = progress_harness(app);
    h.run_steps(5);
    h.get_by_label("キャンセル").click();
    h.run_steps(2);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(h.state().export_pending.as_ref().unwrap().cancel_requested);
    assert!(matches!(
        h.state().combine_session,
        Some(CombineSession::Writing { .. })
    ));
    tx.send(ExportEvent::Cancelled).unwrap();
    let ctx = h.ctx.clone();
    h.state_mut().poll_export_pending(&ctx);
    assert!(h.state().combine_session.is_none());
    assert!(h.state().export_pending.is_none());
}

#[test]
fn image_combine_completed_progress_close_and_x_release_unfinished_owner() {
    for label in ["閉じる", "Close window"] {
        let mut app = crate::app::setup_app_for_test();
        let (mut p, _tx) = pending();
        p.end = Some(ExportEnd::Done);
        p.errors.push(ExportFailure {
            label: "image".into(),
            message: "failed".into(),
        });
        app.export_pending = Some(p);
        app.combine_session = Some(CombineSession::Writing { request: 1 });
        let mut h = progress_harness(app);
        h.run_steps(5);
        h.get_by_label(label).click();
        h.run_steps(2);
        assert!(h.state().combine_session.is_none());
        assert!(h.state().export_pending.is_none());
    }
}

struct WindowState {
    dialog: Option<CombineDialogState>,
    writes: usize,
    cancels: usize,
}
fn harness(size: egui::Vec2, scale: f32, band: bool) -> Harness<'static, WindowState> {
    let mut ready = false;
    crate::test_ui::builder()
        .with_size(size)
        .with_context_setup(|ctx| {
            crate::ui_fonts::configure_fonts(ctx);
            crate::ime_focus::install_ime_input_policy(ctx);
        })
        .build_ui_state(
            move |ui, state: &mut WindowState| {
                if !ready {
                    crate::settings::apply_ui_scale_factor(ui.ctx(), scale);
                    ready = true;
                    return;
                }
                let dialog = state
                    .dialog
                    .get_or_insert_with(|| fixture_state(ui.ctx(), band));
                let action = draw_window(ui.ctx(), dialog, &[]);
                state.writes += usize::from(action.write);
                state.cancels += usize::from(action.cancel);
            },
            WindowState {
                dialog: None,
                writes: 0,
                cancels: 0,
            },
        )
}

#[test]
fn image_combine_small_viewports_keep_controls_and_settings_scroll_to_end() {
    for size in [egui::vec2(1093.0, 614.0), egui::vec2(1366.0, 728.0)] {
        for scale in [1.0, 2.0] {
            let mut h = harness(size, scale, true);
            h.run_steps(12);
            let bounds = h.ctx.content_rect();
            for label in ["Close window", "書き出す", "キャンセル", "保存..."] {
                assert!(
                    bounds.contains_rect(h.get_by_label(label).rect()),
                    "{size:?}/{scale}: {label}"
                );
            }
            let before = h
                .ctx
                .data(|data| {
                    data.get_temp::<ScrollLayout>(egui::Id::unique("image_combine_scroll_layout"))
                })
                .unwrap();
            before.assert_height(HeightExpectation::Scrolls, bounds, h.ctx.pixels_per_point());
            h.event(egui::Event::PointerMoved(before.inner.center()));
            h.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -100000.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
            });
            h.run_steps(32);
            let after = h
                .ctx
                .data(|data| {
                    data.get_temp::<ScrollLayout>(egui::Id::unique("image_combine_scroll_layout"))
                })
                .unwrap();
            after.assert_scrolled_to_end(before, h.ctx.pixels_per_point());
            assert!(
                after
                    .inner
                    .intersects(h.get_by_label("指定のフォルダ").rect())
            );
            h.get_by_label("キャンセル").click();
            h.run_steps(2);
            assert!(h.state().cancels > 0);
        }
    }
}

#[test]
fn image_combine_loading_completion_preserves_label_click_target() {
    for size in [egui::vec2(1093.0, 614.0), egui::vec2(1600.0, 900.0)] {
        let mut h = harness(size, 1.0, false);
        h.run_steps(6);
        let dialog = h.state_mut().dialog.as_mut().unwrap();
        dialog.preview = None;
        for item in &mut dialog.items {
            item.status = ItemStatus::Loading;
        }
        assert!(footer_error(dialog).is_none());
        h.run_steps(3);
        let loading_rect = h
            .get_by_role_and_label(egui::accesskit::Role::TextInput, "ラベル")
            .rect();
        for item in &mut h.state_mut().dialog.as_mut().unwrap().items {
            item.status = ItemStatus::Loaded((600, 800));
        }
        let position = loading_rect.center();
        h.event(egui::Event::PointerMoved(position));
        h.step();
        assert_eq!(
            h.get_by_role_and_label(egui::accesskit::Role::TextInput, "ラベル")
                .rect(),
            loading_rect,
            "{size:?}: loading completion must not move the input field"
        );
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            h.step();
        }
        assert_eq!(
            h.ctx.memory(|memory| memory.focused()),
            Some(egui::Id::unique("image_combine_label"))
        );
    }
}

#[test]
fn image_combine_enter_never_exports_even_with_focused_export_button() {
    let mut h = harness(egui::vec2(1093.0, 614.0), 1.0, false);
    h.run_steps(6);
    for (role, label) in [
        (egui::accesskit::Role::TextInput, "名前"),
        (egui::accesskit::Role::TextInput, "ラベル"),
        (egui::accesskit::Role::Button, "書き出す"),
    ] {
        h.get_by_role_and_label(role, label).focus();
        h.step();
        h.event(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: Some(egui::Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(2);
        h.event(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: Some(egui::Key::Enter),
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(2);
        assert_eq!(h.state().writes, 0);
    }
    h.get_by_label("書き出す").click();
    h.run_steps(2);
    assert_eq!(h.state().writes, 1);
}

#[test]
fn image_combine_fast_preview_drag_keeps_press_owner_and_label() {
    let mut h = harness(egui::vec2(1093.0, 614.0), 1.0, true);
    h.run_steps(8);
    let cards = h
        .ctx
        .data(|data| {
            data.get_temp::<Vec<(usize, egui::Rect)>>(egui::Id::unique(
                "image_combine_preview_cards",
            ))
        })
        .unwrap();
    let source = cards.iter().find(|(id, _)| *id == 2).unwrap().1.center();
    let target = cards.iter().find(|(id, _)| *id == 0).unwrap().1;
    let destination = egui::pos2(target.left() + target.width() * 0.2, target.center().y);
    h.event(egui::Event::PointerMoved(source));
    h.step();
    h.event(egui::Event::PointerButton {
        pos: source,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    h.event(egui::Event::PointerMoved(destination));
    h.step();
    h.event(egui::Event::PointerButton {
        pos: destination,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(2);
    let dialog = h.state().dialog.as_ref().unwrap();
    assert_eq!(dialog.items[0].id, 2);
    assert_eq!(
        dialog.items[0].spec.label_override.as_deref(),
        Some("商品3")
    );
    assert_eq!(dialog.selected, Some(2));
}

#[test]
fn image_combine_names_counts_ratios_settings_and_preset_limits() {
    assert_eq!(parse_counts("2-2", 4).unwrap(), [2, 2]);
    assert!(parse_counts("1-2", 4).is_err());
    assert!(parse_counts("0-4", 4).is_err());
    assert_eq!(parse_ratio("4:5").unwrap(), (4, 5));
    assert!(parse_ratio("0:5").is_err());
    let defaults: crate::settings::Settings = serde_json::from_str("{}").unwrap();
    assert!(defaults.combine_options.is_none());
    assert!(defaults.combine_presets.is_empty());
    let ctx = egui::Context::default();
    let state = fixture_state(&ctx, true);
    let preset = CombinePreset {
        name: "商品用".into(),
        options: state.options.clone(),
    };
    let wire = serde_json::to_string(&preset).unwrap();
    assert!(!wire.contains("label_override"));
    assert!(!wire.contains("商品3"));
    assert!(presets::checked_name("", &[]).is_err());
    assert!(presets::checked_name(&"a".repeat(41), &[]).is_err());
    let full = (0..50)
        .map(|n| CombinePreset {
            name: n.to_string(),
            options: Default::default(),
        })
        .collect::<Vec<_>>();
    assert!(presets::checked_name("new", &full).is_err());
    assert_eq!(presets::checked_name(" 0 ", &full).unwrap(), "0");
}
