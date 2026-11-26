use super::*;
use crate::export_dialog::{
    ExportEnd, ExportEvent, ExportFailure, ExportOrigin, ExportPending, ExportSuccess,
};
use crate::test_ui::scroll_layout::{HeightExpectation, ScrollLayout};
use egui_kittest::{Harness, kittest::Queryable};
use std::sync::{Arc, atomic::AtomicBool, mpsc};

#[test]
fn image_combine_status_loading_invalid_split_valid_and_failed() {
    let ctx = egui::Context::default();
    let mut state = fixture_state(&ctx, false);
    state.items = (0..8)
        .map(|id| {
            let mut item = state.items[id % 4].clone();
            item.id = id;
            item.status = ItemStatus::Loading;
            item
        })
        .collect();
    assert_eq!(status_text(&state), "読み込み中 0/8");
    assert!(footer_error(&state).is_none());
    state.items[0].status = ItemStatus::Loaded((600, 800));
    state.items[1].status = ItemStatus::Failed("decode failed".into());
    assert_eq!(status_text(&state), "読み込み中 2/8");
    for item in &mut state.items {
        item.status = ItemStatus::Loaded((600, 800));
    }
    state.options.split = Split::Counts(vec![3, 3]);
    state.split_text = "3-3".into();
    let invalid = "出力: 計算できません (下の理由を直してください)";
    assert_eq!(status_text(&state), invalid);
    assert!(footer_error(&state).is_some());
    state.options.split = Split::Auto;
    state.split_text.clear();
    let size = state.validation().result.as_ref().unwrap().canvas_size;
    assert!(status_text(&state).starts_with(&format!("出力 {} × {} px", size.0, size.1)));
    assert!(footer_error(&state).is_none());
    state.items[0].status = ItemStatus::Failed("decode failed".into());
    assert!(!state.ready());
    assert_eq!(status_text(&state), invalid);
    assert!(
        footer_error(&state)
            .unwrap()
            .starts_with("読めない画像を外してください:")
    );
    state.reopened = Some(ReopenedOutput {
        path: PathBuf::from("C:/Pictures/combined.png"),
        format: CombineFormat::Png,
        replace: true,
        has_edits: false,
    });
    assert_eq!(
        status_text(&state),
        format!("{invalid}\n元の結合画像を置き換えます (元に戻せません): combined.png")
    );
}

#[test]
fn image_combine_preview_checker_follows_delivered_options_while_draft_changes() {
    let ctx = egui::Context::default();
    let checker = crate::ui_helpers::checker_texture(&ctx);
    let mut state = fixture_state(&ctx, false);
    let (_commands, events, _) = connection(&mut state);
    for (format, transparent, expected_checker) in [
        (CombineFormat::Png, false, false),
        (CombineFormat::Png, true, true),
        (
            CombineFormat::Webp {
                quality: 90,
                lossless: true,
            },
            false,
            false,
        ),
        (
            CombineFormat::Webp {
                quality: 90,
                lossless: true,
            },
            true,
            true,
        ),
        (CombineFormat::Jpeg { quality: 95 }, false, false),
        (CombineFormat::Jpeg { quality: 95 }, true, false),
    ] {
        state.options.format = format;
        state.options.transparent_background = transparent;
        state.generation += 1;
        let layout = state.validation().result.as_ref().unwrap().clone();
        let rgba = create_canvas(&layout, &state.options, 0.1).unwrap();
        let revision = state.validation().revision;
        state.requested = Some(SubmittedPreview {
            revision,
            spec: state.spec(),
        });
        events
            .send(Event::Rendered {
                generation: state.generation,
                rgba,
                layout,
                not_drawn: 0,
            })
            .unwrap();
        // Change the draft before delivery. Painting must use the submitted
        // options, then retain that decision after another request is pending.
        state.options.format = CombineFormat::Png;
        state.options.transparent_background = !expected_checker;
        state.poll(&ctx);
        state.invalidate_and_request(&ctx);
        let output = crate::test_ui::headless_paint_output(ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| draw_preview(ui, &mut state, 240.0));
            },
        ));
        let meshes = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) => Some(mesh),
                _ => None,
            })
            .collect::<Vec<_>>();
        let checks = meshes
            .iter()
            .filter(|mesh| mesh.texture_id == checker.id())
            .collect::<Vec<_>>();
        assert_eq!(
            checks.len(),
            usize::from(expected_checker),
            "{format:?}, {transparent}"
        );
        if let Some(check) = checks.first() {
            let image = meshes
                .iter()
                .find(|mesh| mesh.texture_id == state.preview.as_ref().unwrap().texture.id())
                .unwrap();
            assert_eq!(check.calc_bounds(), image.calc_bounds());
            let uv = crate::ui_helpers::checker_quad_uvs(image.calc_bounds().size());
            assert_eq!(check.vertices[0].uv, uv[0]);
            assert!(check.vertices.iter().any(|vertex| vertex.uv == uv[2]));
        }
    }
}

#[test]
fn image_combine_app_state_storage_is_one_pointer() {
    fn storage<T>(_: fn(&App) -> &T) -> usize {
        std::mem::size_of::<T>()
    }
    assert_eq!(
        storage(|app| &app.image_combine),
        std::mem::size_of::<usize>()
    );
}

#[test]
fn image_combine_index_failure_reaches_app_feedback_once_without_retry() {
    let mut app = crate::app::setup_app_for_test();
    let key = crate::adjustment_db::normalize_path(&app.tmp.path().join("combined.png"));
    app.image_combine
        .index
        .set_keys_for_test([key.clone()].into());
    app.image_combine
        .index
        .fail_worker_start_for_test("injected index worker failure");
    let ctx = egui::Context::default();
    app.draw_image_combine_dialog(&ctx);
    assert!(
        app.fs_feedback_toast
            .as_ref()
            .unwrap()
            .0
            .contains("injected index worker failure")
    );
    assert!(app.image_combine.index.contains_or_unloaded(&key));
    app.fs_feedback_toast = None;
    app.draw_image_combine_dialog(&ctx);
    assert!(app.fs_feedback_toast.is_none());
    assert!(app.image_combine.index.contains_or_unloaded(&key));
}

#[test]
fn image_combine_reopen_keyed_lookup_routes_mounted_snapshot() {
    let mut app = crate::app::setup_app_for_test();
    app.settings.bake_stage_export_batch = crate::bake_stage::BakeStage::Edits;
    let source = CombineSourceRef::File {
        path: app.tmp.path().join("mounted.png"),
    };
    let key = resolver::source_page_key(&source);
    app.items = vec![crate::grid_item::GridItem::Image(
        crate::image_combine::store::source_path(&source).into(),
    )];
    app.adjustment_page_params.insert(
        0,
        crate::adjustment::AdjustParams {
            brightness: 29.0,
            ..Default::default()
        },
    );
    let prepared = resolver::read_source_preparation(app.tmp.path(), &source).unwrap();
    let ctx = egui::Context::default();
    let mut state = fixture_state(&ctx, false);
    let (commands, _events, _cancel) = connection(&mut state);
    state.pending = VecDeque::from([
        (
            0,
            PreparationTarget::ReopenLookup(resolver::ReopenLookup::new(
                &app,
                std::iter::once(key),
            )),
        ),
        (
            0,
            PreparationTarget::Reopen {
                source,
                annotation_space: None,
                composition_identity: None,
                mounted_idx: None,
                prepared: Ok(prepared),
            },
        ),
    ]);
    while !state.pending.is_empty() {
        app.advance_image_combine_preparation(&mut state);
    }
    let Command::Prepare { item, .. } = commands.try_recv().unwrap() else {
        panic!("prepared source");
    };
    assert_eq!(item.edits.params.brightness, 29.0);
    assert!(commands.try_recv().is_err());
}

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
    app.image_combine.session = Some(CombineSession::Dialog(Box::new(state)));
    assert_eq!(app.modal_dialog_block_reason(), Some("image_combine"));
    assert!(app.document_open_modal_admission_blocked());
    app.open_image_combine_dialog(&ctx);
    assert!(matches!(
        app.image_combine.session,
        Some(CombineSession::Dialog(_))
    ));
    app.image_combine.session = None;
    assert!(cancel.load(Ordering::Relaxed));
    assert!(!app.document_open_modal_admission_blocked());
    app.image_combine.session = Some(CombineSession::Writing { request: 1 });
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
fn image_combine_context_targets_are_checked_first_else_clicked_independent_of_cursor() {
    use crate::grid_item::GridItem;
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![
        GridItem::Image("C:/images/cursor.png".into()),
        GridItem::Video("C:/images/clicked.mp4".into()),
        GridItem::Image("C:/images/clicked.png".into()),
    ];
    app.visible_indices = vec![0, 1, 2];
    app.move_grid_cursor(Some(0), crate::app::GridCursorIntent::Automatic);
    assert!(app.image_combine_context_targets(Some(1), false).is_empty());
    let targets = app.image_combine_context_targets(Some(2), false);
    assert!(matches!(
        targets.front(),
        Some(ExportBatchTarget::Item { idx: 2, .. })
    ));
    app.checked.insert(0);
    let targets = app.image_combine_context_targets(Some(1), false);
    assert!(matches!(
        targets.front(),
        Some(ExportBatchTarget::Item { idx: 0, .. })
    ));
    app.checked.clear();
    app.checked.insert(1);
    assert!(app.image_combine_context_targets(Some(2), false).is_empty());
    app.checked.insert(0);
    assert!(app.image_combine_context_targets(None, true).is_empty());
    assert_eq!(app.selected, Some(0));
}

#[test]
fn image_combine_context_stack_expansion_enforces_the_target_limit() {
    use crate::filename_stack::{StackMember, StackView};
    let mut app = crate::app::setup_app_for_test();
    for count in [100, 101] {
        let view = StackView::build(
            PathBuf::from("C:/images"),
            Vec::new(),
            Vec::new(),
            (0..count)
                .map(|n| StackMember {
                    path: PathBuf::from(format!("C:/images/post_{n:03}.png")),
                    mtime: 0,
                    size: Some(8),
                    sort_meta: crate::settings::ListingSortMetadata::new(0, Some(8)),
                    is_video: false,
                })
                .collect(),
            '_',
            crate::settings::SortOrder::FileName,
        );
        app.items = view.materialize_aggregated().0;
        app.stack_view = Some(Arc::new(view));
        let targets = app.image_combine_context_targets(Some(0), false);
        assert_eq!(targets.len(), count);
        assert!(
            targets
                .iter()
                .all(|target| matches!(target, ExportBatchTarget::StackMember(_)))
        );
        let reason = app.combine_disabled_reason_for_count(targets.len());
        if count == 100 {
            assert!(reason.is_none());
        } else {
            assert!(reason.unwrap().contains("101 枚"));
        }
    }
}

#[test]
fn image_combine_escape_closes_and_late_worker_result_has_no_receiver() {
    let ctx = egui::Context::default();
    crate::ime_focus::install_ime_input_policy(&ctx);
    let mut app = crate::app::setup_app_for_test();
    let mut state = fixture_state(&ctx, false);
    let (_commands, events, cancel) = connection(&mut state);
    app.image_combine.session = Some(CombineSession::Dialog(Box::new(state)));
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
    assert!(app.image_combine.session.is_none());
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
    app.items = state
        .items
        .iter()
        .map(|item| crate::grid_item::GridItem::Image(source_path(&item.spec.source).to_path_buf()))
        .collect();
    app.move_grid_cursor(Some(3), crate::app::GridCursorIntent::Automatic);
    app.checked.insert(3);
    let grid_selection = (app.selected, app.checked.clone());
    state.selected_item_id = Some(0);
    let (commands, _events, _cancel) = connection(&mut state);
    let count = state.items.len();
    state.pending.push_back((
        0,
        PreparationTarget::Selection(ExportBatchTarget::Item {
            idx: usize::MAX,
            page_key: "missing".into(),
        }),
    ));
    app.advance_image_combine_preparation(&mut state);
    assert_eq!(state.items.len(), count);
    assert!(matches!(state.items[0].status, ItemStatus::Failed(_)));
    assert!(!state.can_write());
    state.remove_failed();
    assert_eq!(state.items.len(), count - 1);
    assert_eq!(state.selected_item_id, Some(1));
    assert_eq!((app.selected, app.checked.clone()), grid_selection);
    assert!(matches!(commands.try_recv().unwrap(),Command::Remove{ids} if ids==vec![0]));
}

#[test]
fn image_combine_preview_generation_discards_stale_results_and_reorder_keeps_labels() {
    let ctx = egui::Context::default();
    let mut state = fixture_state(&ctx, true);
    let (_commands, events, _) = connection(&mut state);
    state.generation = 8;
    let revision = state.validation().revision;
    state.requested = Some(SubmittedPreview {
        revision,
        spec: state.spec(),
    });
    let layout = state.validation().result.as_ref().unwrap().clone();
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
    app.image_combine.session = Some(CombineSession::Writing { request: 42 });
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
    assert!(app.image_combine.session.is_none());
    assert!(
        matches!(&app.fs_feedback_toast_action,Some(crate::feedback::FeedbackAction::OpenInViewer(p)) if *p==path)
    );
    assert!(app.post_operation_selection.is_some());
    assert_eq!(app.external_rescan_pending.as_ref().unwrap().folder, folder);
    assert!(matches!(
        app.external_rescan_pending.as_ref().unwrap().cause,
        crate::app::book_append_refresh::FolderRefreshCause::Operation
    ));
    app.fs_feedback_toast_action = None;
    app.folder_refresh_pending = None;
    app.poll_export_pending(&ctx);
    app.finish_combine_write(&ctx);
    assert!(app.fs_feedback_toast_action.is_none());
    assert!(app.folder_refresh_pending.is_none());
    assert!(
        !app.external_rescan_pending
            .as_ref()
            .unwrap()
            .restart_requested
    );
}

#[test]
fn image_combine_recipe_only_failure_adopts_output_and_requests_index_reload() {
    let mut app = crate::app::setup_app_for_test();
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("joined.png");
    let stale = "old-output-key".to_string();
    app.image_combine
        .index
        .set_keys_for_test([stale.clone()].into());
    app.image_combine.session = Some(CombineSession::Writing { request: 8 });
    let (pending, tx) = pending();
    app.export_pending = Some(pending);
    tx.send(ExportEvent::Completed(ExportSuccess {
        label: "joined.png".into(),
        path: output.clone(),
    }))
    .unwrap();
    tx.send(ExportEvent::Failed(ExportFailure {
        label: "構成の保存".into(),
        message: "構成を保存できませんでした。やり直しは使えません".into(),
    }))
    .unwrap();
    tx.send(ExportEvent::AllDone).unwrap();
    let ctx = egui::Context::default();
    app.poll_export_pending(&ctx);
    assert!(app.image_combine.session.is_none());
    assert_eq!(
        app.export_pending.as_ref().unwrap().successes[0].path,
        output
    );
    assert_eq!(
        app.export_pending.as_ref().unwrap().errors[0].label,
        "構成の保存"
    );
    assert!(
        matches!(&app.fs_feedback_toast_action, Some(crate::feedback::FeedbackAction::OpenInViewer(path)) if path == &output)
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.image_combine.index.contains_or_unloaded(&stale) {
        assert!(app.image_combine.index.poll(temp.path(), &ctx).is_none());
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn image_combine_completion_in_other_folder_does_not_refresh_or_select() {
    let ctx = egui::Context::default();
    let mut app = crate::app::setup_app_for_test();
    let books = app.tmp.path().join("books");
    app.current_folder = Some(books.join("current"));
    app.current_folder_last_mtime = Some(std::time::SystemTime::now());
    app.settings.book_root = Some(books);
    let (p, tx) = pending();
    app.export_pending = Some(p);
    app.image_combine.session = Some(CombineSession::Writing { request: 1 });
    tx.send(ExportEvent::Completed(ExportSuccess {
        label: "combined".into(),
        path: app.tmp.path().join("other/combined.png"),
    }))
    .unwrap();
    tx.send(ExportEvent::AllDone).unwrap();
    app.poll_export_pending(&ctx);
    assert!(app.image_combine.session.is_none());
    assert!(app.post_operation_selection.is_none());
    assert!(app.external_rescan_pending.is_none());
    assert!(app.folder_refresh_pending.is_none());
}

#[test]
fn image_combine_poll_completion_refreshes_and_selects_output_under_book_root() {
    use crate::grid_item::GridItem;
    for relative in ["", "book", "book/nested"] {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        let books = app.tmp.path().join("books");
        // Joining an empty component adds a trailing separator; the accepted
        // directory and the output's parent must use the same physical spelling.
        let folder = if relative.is_empty() {
            books.clone()
        } else {
            books.join(relative)
        };
        std::fs::create_dir_all(&folder).unwrap();
        app.settings.book_root = Some(books);
        let first = folder.join("first.png");
        let output = folder.join("combined.png");
        for path in [&first, &output] {
            image::RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 255]))
                .save(path)
                .unwrap();
        }
        app.current_folder = Some(folder.clone());
        app.current_folder_last_mtime = Some(std::time::SystemTime::UNIX_EPOCH);
        app.items = vec![GridItem::Image(first)];
        app.visible_indices = vec![0];
        app.thumbnails = vec![crate::grid_item::ThumbnailState::Pending];
        app.selected = Some(0);
        app.checked.insert(0);
        let generation_before_refresh = app.items_generation;
        assert!(app.current_favorite_target().is_none());
        let (p, tx) = pending();
        app.export_pending = Some(p);
        app.image_combine.session = Some(CombineSession::Writing { request: 1 });
        tx.send(ExportEvent::Completed(ExportSuccess {
            label: "combined".into(),
            path: output.clone(),
        }))
        .unwrap();
        tx.send(ExportEvent::AllDone).unwrap();
        app.poll_export_pending(&ctx);
        assert!(app.export_pending.is_none());
        assert!(crate::folder_tree::path_eq(
            output.parent().unwrap(),
            &folder
        ));
        let scan = app
            .external_rescan_pending
            .as_ref()
            .expect("matching displayed folder must be rescanned");
        assert_eq!(scan.folder, folder);
        assert!(matches!(
            scan.cause,
            crate::app::book_append_refresh::FolderRefreshCause::Operation
        ));
        assert!(!scan.restart_requested);
        crate::app::tests::phase_c_support::wait_for_external_rescan(&mut app);
        app.finish_history_restore_preparation_for_test();
        assert!(
            app.items_generation > generation_before_refresh,
            "{relative}: operation refresh must install a new listing"
        );
        let index = app.items.iter().position(|item| {
            matches!(item, GridItem::Image(path) if crate::folder_tree::path_eq(path, &output))
        }).expect("operation rescan must install the output");
        assert_eq!(app.selected, Some(index), "{relative}");
        // The shared operation-selection policy puts only the cursor on a
        // single output and clears the previous checkmarks.
        assert!(
            app.checked.is_empty(),
            "{relative}: one output has no checkmark"
        );
        let selection = app
            .post_operation_selection
            .as_ref()
            .expect("applied Known request is retained");
        assert!(
            selection.has_applied(),
            "{relative}: Known selection must be applied during adoption"
        );
        assert!(
            selection.still_owns_selection(Some(&output), &[]),
            "{relative}: applied output must own the cursor with nothing checked"
        );
        let generation = app.items_generation;
        app.poll_export_pending(&ctx);
        app.finish_combine_write(&ctx);
        app.consume_folder_refresh_pending(&ctx);
        assert!(app.external_rescan_pending.is_none());
        assert_eq!(app.items_generation, generation);
    }
}

fn image_combine_deliver_preview(
    state: &mut CombineDialogState,
    commands: &mpsc::Receiver<Command>,
    events: &mpsc::Sender<Event>,
    ctx: &egui::Context,
) -> RenderSpec {
    let Command::Render { generation, spec } = commands.try_recv().expect("render submitted")
    else {
        panic!("expected a render command");
    };
    events
        .send(Event::Rendered {
            generation,
            rgba: image::RgbaImage::new(1, 1),
            layout: state.validation().result.as_ref().unwrap().clone(),
            not_drawn: 0,
        })
        .unwrap();
    state.poll(ctx);
    assert!(
        state.can_write(),
        "repaired draft must be exportable after preview delivery"
    );
    spec
}

#[test]
fn image_combine_invalid_manual_split_margin_change_then_repair_submits_preview() {
    let ctx = egui::Context::default();
    let mut state = fixture_state(&ctx, false);
    let (commands, events, _) = connection(&mut state);
    state.options.split = Split::Counts(vec![2, 2]);
    state.split_text = "2-2".into();
    state.invalidate_and_request(&ctx);
    image_combine_deliver_preview(&mut state, &commands, &events, &ctx);
    let sent_revision = state.requested.as_ref().unwrap().revision;
    state.split_text = "2-".into();
    state.invalidate_and_request(&ctx);
    state.options.margin = 40;
    state.invalidate_and_request(&ctx);
    assert!(!state.can_write());
    assert!(commands.try_recv().is_err());
    assert_eq!(state.requested.as_ref().unwrap().revision, sent_revision);
    state.split_text = "2-2".into();
    state.invalidate_and_request(&ctx);
    assert!(!state.can_write());
    let sent = image_combine_deliver_preview(&mut state, &commands, &events, &ctx);
    assert_eq!(sent.options.margin, 40);
    assert_eq!(sent.options.split, Split::Counts(vec![2, 2]));
    state.invalidate_and_request(&ctx);
    assert!(commands.try_recv().is_err());
}

#[test]
fn image_combine_invalid_custom_ratio_margin_change_then_repair_submits_preview() {
    let ctx = egui::Context::default();
    let mut state = fixture_state(&ctx, false);
    let (commands, events, _) = connection(&mut state);
    state.options.canvas_ratio = Some((5, 4));
    state.custom_ratio_text = Some("5:4".into());
    state.invalidate_and_request(&ctx);
    image_combine_deliver_preview(&mut state, &commands, &events, &ctx);
    let sent_revision = state.requested.as_ref().unwrap().revision;
    state.custom_ratio_text = Some("5:".into());
    state.invalidate_and_request(&ctx);
    state.options.margin = 40;
    state.invalidate_and_request(&ctx);
    assert!(!state.can_write());
    assert!(commands.try_recv().is_err());
    assert_eq!(state.requested.as_ref().unwrap().revision, sent_revision);
    state.custom_ratio_text = Some("5:4".into());
    state.invalidate_and_request(&ctx);
    assert!(!state.can_write());
    let sent = image_combine_deliver_preview(&mut state, &commands, &events, &ctx);
    assert_eq!(sent.options.margin, 40);
    assert_eq!(sent.options.canvas_ratio, Some((5, 4)));
    state.invalidate_and_request(&ctx);
    assert!(commands.try_recv().is_err());
}

#[test]
fn image_combine_poll_disconnect_cancel_and_failed_completion_release_session() {
    for terminal in [None, Some(ExportEnd::Cancelled), Some(ExportEnd::Done)] {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        app.image_combine.session = Some(CombineSession::Writing { request: 1 });
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
        assert!(app.image_combine.session.is_none());
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
    app.image_combine.session = Some(CombineSession::Writing { request: 1 });
    tx.send(ExportEvent::Completed(ExportSuccess {
        label: "combined".into(),
        path: app.tmp.path().join("combined.png"),
    }))
    .unwrap();
    drop(tx);
    app.poll_export_pending(&ctx);
    assert!(app.image_combine.session.is_none());
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
    app.image_combine.session = Some(CombineSession::Writing { request: 1 });
    let mut h = progress_harness(app);
    h.run_steps(5);
    h.get_by_label("キャンセル").click();
    h.run_steps(2);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(h.state().export_pending.as_ref().unwrap().cancel_requested);
    assert!(matches!(
        h.state().image_combine.session,
        Some(CombineSession::Writing { .. })
    ));
    tx.send(ExportEvent::Cancelled).unwrap();
    let ctx = h.ctx.clone();
    h.state_mut().poll_export_pending(&ctx);
    assert!(h.state().image_combine.session.is_none());
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
        app.image_combine.session = Some(CombineSession::Writing { request: 1 });
        let mut h = progress_harness(app);
        h.run_steps(5);
        h.get_by_label(label).click();
        h.run_steps(2);
        assert!(h.state().image_combine.session.is_none());
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
            let [header, body, footer] = h
                .ctx
                .data(|data| {
                    data.get_temp::<[egui::Rect; 3]>(egui::Id::unique("image_combine_fixed_layout"))
                })
                .unwrap();
            assert!(
                bounds.contains_rect(header),
                "{size:?}/{scale}: fixed header"
            );
            assert!(
                bounds.contains_rect(footer),
                "{size:?}/{scale}: fixed footer"
            );
            assert!(
                body.top() >= header.bottom(),
                "{size:?}/{scale}: header={header:?}, body={body:?}"
            );
            assert!(
                body.bottom() <= footer.top(),
                "{size:?}/{scale}: body={body:?}, footer={footer:?}"
            );
            let window = h
                .ctx
                .memory(|memory| memory.area_rect(egui::Id::unique("image_combine_dialog")))
                .unwrap();
            let expected_height = 680.0_f32.min(bounds.height() - 40.0);
            assert!(
                (window.height() - expected_height).abs() <= 1.0,
                "{size:?}/{scale}: window={window:?}, bounds={bounds:?}"
            );
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
fn image_combine_reopen_replace_footer_notice_is_fixed_and_tracks_replace() {
    for size in [egui::vec2(1093.0, 614.0), egui::vec2(1366.0, 728.0)] {
        for scale in [1.0, 2.0] {
            let mut h = harness(size, scale, false);
            h.run_steps(12);
            let plain_body_height = h
                .ctx
                .data(|data| {
                    data.get_temp::<[egui::Rect; 3]>(egui::Id::unique("image_combine_fixed_layout"))
                })
                .unwrap()[1]
                .height();
            h.state_mut().dialog.as_mut().unwrap().reopened = Some(ReopenedOutput {
                path: PathBuf::from("C:/Pictures/元の結合画像_商品一覧.jpg"),
                format: CombineFormat::Jpeg { quality: 95 },
                replace: true,
                has_edits: false,
            });
            h.run_steps(12);
            let notice = "元の結合画像を置き換えます (元に戻せません): 元の結合画像_商品一覧.jpg";
            let rect = h.get_by_label_contains(notice).rect();
            let bounds = h.ctx.content_rect();
            let [header, body, footer] = h
                .ctx
                .data(|data| {
                    data.get_temp::<[egui::Rect; 3]>(egui::Id::unique("image_combine_fixed_layout"))
                })
                .unwrap();
            assert!(
                bounds.contains_rect(header),
                "{size:?}/{scale}: fixed header"
            );
            assert!(
                bounds.contains_rect(footer),
                "{size:?}/{scale}: fixed footer"
            );
            assert!(
                body.top() >= header.bottom(),
                "{size:?}/{scale}: header={header:?}, body={body:?}"
            );
            assert!(
                body.bottom() <= footer.top(),
                "{size:?}/{scale}: body={body:?}, footer={footer:?}"
            );
            let window = h
                .ctx
                .memory(|memory| memory.area_rect(egui::Id::unique("image_combine_dialog")))
                .unwrap();
            let expected_height = 680.0_f32.min(bounds.height() - 40.0);
            assert!(
                (window.height() - expected_height).abs() <= 1.0,
                "{size:?}/{scale}: window={window:?}, bounds={bounds:?}"
            );
            assert!(
                body.height() < plain_body_height,
                "notice must shrink the body rather than expand the Window"
            );
            assert!(
                bounds.contains_rect(rect),
                "{size:?}/{scale}: replacement notice"
            );
            for label in ["書き出す", "キャンセル", "Close window", "保存..."] {
                assert!(
                    bounds.contains_rect(h.get_by_label(label).rect()),
                    "{size:?}/{scale}: {label}"
                );
            }
            let scroll = h
                .ctx
                .data(|data| {
                    data.get_temp::<ScrollLayout>(egui::Id::unique("image_combine_scroll_layout"))
                })
                .unwrap();
            assert!(
                !scroll.inner.intersects(rect),
                "replacement notice belongs to the fixed footer"
            );
            let state = h.state_mut().dialog.as_mut().unwrap();
            assert!(status_text(state).starts_with("出力 "));
            state.reopened.as_mut().unwrap().replace = false;
            h.run_steps(12);
            let restored_body_height = h
                .ctx
                .data(|data| {
                    data.get_temp::<[egui::Rect; 3]>(egui::Id::unique("image_combine_fixed_layout"))
                })
                .unwrap()[1]
                .height();
            assert!(restored_body_height > body.height());
            assert!(
                h.query_by_label_contains("元の結合画像を置き換えます")
                    .is_none()
            );
        }
    }
}

#[test]
fn image_combine_validation_cache_tracks_dimensions_order_labels_and_options() {
    let ctx = egui::Context::default();
    let mut state = fixture_state(&ctx, false);
    assert!(state.validation().result.is_ok());
    assert_eq!(state.layout_searches.get(), 1);
    for change in 0..4 {
        let previous_revision = state.validation().revision;
        match change {
            0 => state.items[0].status = ItemStatus::Loaded((700, 800)),
            1 => state.move_item(2, Some(0)),
            2 => state.items[0].spec.label_override = Some("changed".into()),
            3 => state.options.margin = 40,
            _ => unreachable!(),
        }
        assert!(state.validation().result.is_ok());
        assert!(state.validation().revision > previous_revision);
        assert_eq!(state.layout_searches.get(), change + 2);
        // Selection and output name do not change the composition.
        state.selected_item_id = Some(1);
        state.name.push('a');
        assert!(state.validation().result.is_ok());
        assert_eq!(state.layout_searches.get(), change + 2);
    }
}

#[test]
fn image_combine_100_target_unchanged_paints_reuse_validation_and_submitted_spec() {
    const PAINTS: usize = 30;
    let mut h = harness(egui::vec2(1093.0, 614.0), 1.0, false);
    h.run_steps(6);
    let ctx = h.ctx.clone();
    let state = h.state_mut().dialog.as_mut().unwrap();
    let template = state.items[0].clone();
    state.items = (0..100)
        .map(|id| {
            let mut item = template.clone();
            item.id = id;
            item.name = format!("image-{id}.png");
            item.spec.source = CombineSourceRef::File {
                path: PathBuf::from(format!("C:/Pictures/image-{id}.png")),
            };
            item.status = ItemStatus::Loaded((600 + (id % 7) as u32 * 100, 800));
            item
        })
        .collect();
    assert!(matches!(state.options.split, Split::Auto));
    let (commands, events, _) = connection(state);
    state.layout_searches.set(0);
    state.spec_builds.set(0);
    let cold = std::time::Instant::now();
    state.invalidate_and_request(&ctx);
    let cold_elapsed = cold.elapsed();
    image_combine_deliver_preview(state, &commands, &events, &ctx);
    assert_eq!(state.layout_searches.get(), 1);
    assert_eq!(state.spec_builds.get(), 1);
    // Settle the Window's sizing passes before timing steady-state paints.
    h.run_steps(6);
    let start = std::time::Instant::now();
    for _ in 0..PAINTS {
        h.state_mut()
            .dialog
            .as_mut()
            .unwrap()
            .invalidate_and_request(&ctx);
        h.step();
        h.state_mut()
            .dialog
            .as_mut()
            .unwrap()
            .invalidate_and_request(&ctx);
    }
    let elapsed = start.elapsed();
    let state = h.state().dialog.as_ref().unwrap();
    assert_eq!(
        state.layout_searches.get(),
        1,
        "unchanged paints must not search layouts"
    );
    assert_eq!(
        state.spec_builds.get(),
        1,
        "unchanged pre/post paint checks must not clone specs"
    );
    assert!(commands.try_recv().is_err());
    eprintln!(
        "image_combine 100 targets: cold validation+submit={:.3} ms; {PAINTS} unchanged paints={:.3} ms ({:.3} ms/paint); layout_searches=1 spec_builds=1",
        cold_elapsed.as_secs_f64() * 1000.0,
        elapsed.as_secs_f64() * 1000.0,
        elapsed.as_secs_f64() * 1000.0 / PAINTS as f64,
    );
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
    assert_eq!(dialog.selected_item_id, Some(2));
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
