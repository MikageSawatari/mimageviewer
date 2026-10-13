//! Admission checks run without launching the application or invoking Shell deletion.
#![cfg(test)]

use super::{ActionSurface, App, AppTestEnvForTest, setup_app_for_test};
use crate::book_append_undo::{AppendReceiptBuilder, BookAppendUndoReceipt};
use crate::books::{BookOpIntent, BookOpPending, BookOpResult};
use crate::grid_item::GridItem;

fn receipt(app: &AppTestEnvForTest) -> BookAppendUndoReceipt {
    let root = app.tmp.path().join("producer-books");
    let folder = root.join("book");
    std::fs::create_dir_all(&folder).unwrap();
    let mut builder = AppendReceiptBuilder::begin(&root, &folder, "book".into()).unwrap();
    let page = folder.join("0001_page.jpg");
    std::fs::write(&page, b"generated page").unwrap();
    builder.record(&page).unwrap();
    BookAppendUndoReceipt {
        files: builder.finish().unwrap(),
        operation_id: 1,
        source_context: app.virtual_list_context_id(),
        accepted_revision: 0,
    }
}

fn hold_book_operation(
    app: &mut App,
    intent: BookOpIntent,
) -> std::sync::mpsc::Sender<Result<BookOpResult, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.book_op_pending = Some(BookOpPending { rx, intent });
    tx
}

#[test]
fn section1373_tag_admission_rejects_only_undo_append_before_cache_or_job_submission() {
    for kind in ["source", "delete", "undo"] {
        let mut app = setup_app_for_test();
        let undo_receipt = receipt(&app);
        let path = app.tmp.path().join("other.jpg");
        std::fs::write(&path, b"other source").unwrap();
        app.items = vec![GridItem::Image(path.clone())];
        app.selected = Some(0);
        let intent = match kind {
            "source" => BookOpIntent::SourceMutation,
            "delete" => BookOpIntent::Delete {
                path: undo_receipt.files.folder.clone(),
            },
            _ => BookOpIntent::UndoAppend {
                receipt: undo_receipt,
            },
        };
        let _worker = hold_book_operation(&mut app, intent);
        app.request_tag_add_for_selection("accepted", ActionSurface::MainWindow);
        let key = crate::tags_db::item_key_for_path(&path);
        if kind == "undo" {
            assert!(app.pending_tag_undos.is_empty());
            assert!(app.tags_cache.get(&key).is_none_or(|tags| tags.is_empty()));
            assert!(
                app.tag_write_handle
                    .as_ref()
                    .is_none_or(|handle| !handle.has_unconsumed_batch())
            );
        } else {
            // No poll has run: acceptance must preserve existing optimistic tags
            // and register the normal pending metadata Undo for other book intents.
            assert!(!app.pending_tag_undos.is_empty());
            assert_eq!(
                app.tags_cache.get(&key).unwrap(),
                &vec!["#accepted".to_owned()]
            );
        }
    }
}

#[test]
fn section1373_erase_undo_and_redo_reject_only_pending_undo_append() {
    for undo_append in [false, true] {
        let mut app = setup_app_for_test();
        app.erase_mask_size = [2, 1];
        app.erase_mask = Some(vec![false, false]);
        app.push_undo_snapshot();
        app.erase_mask = Some(vec![true, false]);
        let intent = if undo_append {
            BookOpIntent::UndoAppend {
                receipt: receipt(&app),
            }
        } else {
            BookOpIntent::SourceMutation
        };
        let _worker = hold_book_operation(&mut app, intent);
        assert_eq!(app.undo_erase(), !undo_append);
        if undo_append {
            assert_eq!(app.erase_mask.as_ref().unwrap(), &vec![true, false]);
        } else {
            assert_eq!(app.erase_mask.as_ref().unwrap(), &vec![false, false]);
            assert!(app.redo_erase());
            assert_eq!(app.erase_mask.as_ref().unwrap(), &vec![true, false]);
        }
    }
}

#[test]
fn section1373_spread_shift_acceptance_rejects_only_pending_undo_append() {
    for undo_append in [false, true] {
        let mut app = setup_app_for_test();
        let path = app.tmp.path().join("ordinary.jpg");
        app.items = vec![GridItem::Image(path)];
        let intent = if undo_append {
            BookOpIntent::UndoAppend {
                receipt: receipt(&app),
            }
        } else {
            BookOpIntent::SourceMutation
        };
        let _worker = hold_book_operation(&mut app, intent);
        assert_eq!(app.set_live_spread_shift(0), !undo_append);
        assert_eq!(app.spread_shift_anchor_idx(), (!undo_append).then_some(0));
    }
}

#[cfg(windows)]
#[test]
fn section1373_keyaction_undo_busy_or_invalid_receipt_never_pops_older_metadata() {
    let _serial = crate::key_input::lock_test_input();
    crate::key_input::clear_test_frame();
    for case in ["busy", "invalid"] {
        let mut app = setup_app_for_test();
        let mut append = receipt(&app);
        append.source_context = app.projected_viewer_context_id();
        append.accepted_revision = app.book_append_undo_revision;
        let path = app.tmp.path().join("older.jpg");
        app.meta_undo.push(crate::undo_stack::UndoEntry::Rating {
            changes: vec![crate::undo_stack::RatingChange {
                path_key: path.to_string_lossy().into_owned(),
                source_path: path,
                meta: None,
                xmp_target: None,
                before: 0,
                after: 1,
            }],
            summary: "older".into(),
            origin: None,
        });
        let _worker = if case == "busy" {
            Some(hold_book_operation(&mut app, BookOpIntent::SourceMutation))
        } else {
            append.accepted_revision = append.accepted_revision.wrapping_add(1);
            None
        };
        app.book_append_undo_receipt = Some(append);
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput {
            modifiers: egui::Modifiers::CTRL,
            events: vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL,
            }],
            ..Default::default()
        });
        let owner = crate::keyboard_input::decide_keyboard_owner(
            crate::keyboard_input::KeyboardOwnershipSnapshot {
                viewport: ctx.viewport_id(),
                viewport_focused: true,
                modal: false,
                text_input: None,
                focused_ui: None,
                shortcut_scope: Some(crate::keyboard_input::ShortcutScope::new(
                    ctx.viewport_id(),
                    crate::keyboard_input::ShortcutSurface::Main,
                )),
            },
        );
        crate::keyboard_input::cache_keyboard_owner(&ctx, owner);
        app.handle_meta_undo_keys(&ctx);
        assert_eq!(app.meta_undo.undo_len(), 1, "{case}");
        assert!(
            !app.keymap
                .consume_action(&ctx, crate::keymap::KeyAction::UndoLastAction)
        );
        if case == "invalid" {
            assert!(app.book_append_undo_receipt.is_none());
        }
        let _ = ctx.end_pass();
    }
}

#[test]
fn section1373_uncollected_writer_completion_and_pending_flush_keep_receipt_available() {
    for case in ["local_adjust_completion", "trim_flush"] {
        let mut app = setup_app_for_test();
        let mut append = receipt(&app);
        append.source_context = app.projected_viewer_context_id();
        append.accepted_revision = app.book_append_undo_revision;
        let operation = append.operation();
        app.book_append_undo_receipt = Some(append);
        if case == "local_adjust_completion" {
            app.local_adjust_write_pending.insert(
                "pending.jpg".into(),
                super::LocalAdjustWritePending {
                    generation: 1,
                    idx: 0,
                    sidecar: None,
                },
            );
        } else {
            app.view_trim_save_pending = true;
        }
        assert!(app.try_undo_book_append(&egui::Context::default()));
        assert!(app.book_op_pending.is_none());
        assert!(app.book_append_operation_matches_receipt(operation));
    }
}
